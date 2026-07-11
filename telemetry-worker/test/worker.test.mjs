// Tests for the telemetry worker's dual-write + D1 self-defense behavior.
// Run with: node --test test/
//
// The worker module is plain ESM with injected bindings (env.DB, env.FIREHOSE),
// so it can be exercised without wrangler by passing mocks.
import test from "node:test";
import assert from "node:assert/strict";

import worker from "../src/worker.js";

const EVENT_URL = "https://telemetry.example/v1/event";
const HEALTH_URL = "https://telemetry.example/v1/health";

function makeBody(overrides = {}) {
  return {
    id: "11111111-2222-3333-4444-555555555555",
    event: "onboarding_step",
    version: "0.0.0-test",
    os: "linux",
    arch: "x86_64",
    step: "auth_failed",
    auth_provider: "testprov",
    auth_method: "oauth",
    auth_failure_reason: "callback_timeout",
    ...overrides,
  };
}

function makeDiscoveryBody(overrides = {}) {
  return makeBody({
    event: "discovery",
    event_id: "discovery-event-1",
    session_id: "session-1",
    request_id: "11111111-2222-4333-8444-555555555555",
    phase: "browse",
    category: "payments",
    selected_tool: null,
    outcome: "success",
    failure_reason: null,
    http_status: 200,
    latency_ms: 125,
    response_bytes: 2048,
    result_count: 3,
    query_present: true,
    reason_present: true,
    custom_endpoint: false,
    ...overrides,
  });
}

function postRequest(body, url = EVENT_URL) {
  return new Request(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

// Minimal D1 mock. `plan` lets tests fail specific statements or set the
// reported database size.
function makeDb(plan = {}) {
  const executed = [];
  const sizeAfter = plan.sizeAfter ?? 1000;
  return {
    executed,
    prepare(sql) {
      return {
        bind(...values) {
          return {
            async run() {
              executed.push({ sql, values });
              if (plan.failInserts && /^INSERT/i.test(sql.trim())) {
                throw new Error(plan.failureMessage || "generic transient error");
              }
              return { meta: { changes: 1, size_after: sizeAfter } };
            },
            async all() {
              executed.push({ sql, values });
              return { results: [] };
            },
          };
        },
        async run() {
          executed.push({ sql, values: [] });
          return { meta: { changes: 0, size_after: sizeAfter } };
        },
        async all() {
          executed.push({ sql, values: [] });
          // PRAGMA table_info: report every column the worker may reference.
          if (/table_info\(web_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "path", "referrer", "visitor_id", "utm_source",
                "utm_medium", "utm_campaign", "cta",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info\(discovery_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "request_id", "phase", "category", "selected_tool",
                "outcome", "failure_reason", "http_status", "latency_ms",
                "response_bytes", "result_count", "query_present",
                "reason_present", "custom_endpoint",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info/.test(sql)) {
            return {
              results: [
                "telemetry_id", "event", "version", "os", "arch", "step",
                "auth_provider", "auth_method", "auth_failure_reason",
                "milestone_elapsed_ms", "event_id", "session_id",
                "schema_version", "build_channel", "is_git_checkout", "is_ci",
                "ran_from_cargo", "account_id", "tier", "model_start",
              ].map((name) => ({ name })),
            };
          }
          return { results: [] };
        },
      };
    },
  };
}

function makeFirehose() {
  const points = [];
  return {
    points,
    writeDataPoint(point) {
      points.push(point);
    },
  };
}

function makeCtx() {
  const waited = [];
  return {
    waited,
    waitUntil(promise) {
      waited.push(promise);
    },
  };
}

test("event is dual-written: firehose point + D1 insert", async () => {
  const db = makeDb();
  const firehose = makeFirehose();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db, FIREHOSE: firehose }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, true);
  assert.equal(json.firehose, true);

  assert.equal(firehose.points.length, 1);
  const point = firehose.points[0];
  // index1 = telemetry_id (sampling key)
  assert.deepEqual(point.indexes, ["11111111-2222-3333-4444-555555555555"]);
  // FIREHOSE_SCHEMA blob positions (append-only contract):
  assert.equal(point.blobs[0], "onboarding_step"); // blob1 = event
  assert.equal(point.blobs[7], "auth_failed"); // blob8 = step
  assert.equal(point.blobs[8], "testprov"); // blob9 = auth_provider
  assert.equal(point.blobs[10], "callback_timeout"); // blob11 = auth_failure_reason
  assert.equal(point.blobs.length, 20);
  assert.equal(point.doubles.length, 20);

  assert.ok(db.executed.some(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql)));
});

test("discovery event is validated, firehosed, and persisted to details", async () => {
  const db = makeDb();
  const discoveryFirehose = makeFirehose();
  const response = await worker.fetch(
    postRequest(makeDiscoveryBody()),
    { DB: db, FIREHOSE_DISCOVERY: discoveryFirehose },
    makeCtx(),
  );
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.firehose, true);
  assert.equal(discoveryFirehose.points.length, 1);
  const point = discoveryFirehose.points[0];
  assert.equal(point.blobs[7], "11111111-2222-4333-8444-555555555555");
  assert.equal(point.blobs[8], "browse");
  assert.equal(point.blobs[9], "payments");
  assert.equal(point.blobs[11], "success");
  assert.equal(point.doubles[3], 200);
  assert.equal(point.doubles[4], 125);
  assert.equal(point.doubles[7], 1);

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO discovery_details/.test(sql));
  assert.ok(detailInsert);
  assert.ok(detailInsert.values.includes("11111111-2222-4333-8444-555555555555"));
  assert.ok(detailInsert.values.includes("payments"));
  assert.ok(!detailInsert.values.some((value) => String(value).includes("virtual card")));
});

test("discovery event rejects unknown failure classifications", async () => {
  const response = await worker.fetch(
    postRequest(makeDiscoveryBody({ outcome: "failure", failure_reason: "raw secret error" })),
    { DB: makeDb(), FIREHOSE_DISCOVERY: makeFirehose() },
    makeCtx(),
  );
  assert.equal(response.status, 400);
  assert.match((await response.json()).error, /failure_reason/);
});

test("D1 failure with firehose success degrades to durable:false instead of 500", async () => {
  const db = makeDb({ failInserts: true });
  const firehose = makeFirehose();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db, FIREHOSE: firehose }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, false);
  assert.equal(json.firehose, true);
  assert.equal(firehose.points.length, 1);
});

test("SQLITE_FULL-class insert failure schedules an emergency prune", async () => {
  const db = makeDb({ failInserts: true, failureMessage: "SQLITE_FULL: database or disk is full" });
  const firehose = makeFirehose();
  const ctx = makeCtx();

  await worker.fetch(postRequest(makeBody()), { DB: db, FIREHOSE: firehose }, ctx);
  // The prune is scheduled via ctx.waitUntil; drain it and check DELETEs ran.
  await Promise.all(ctx.waited);

  assert.ok(
    db.executed.some(({ sql }) => /DELETE FROM events/.test(sql)),
    "emergency prune should issue DELETEs after a full-database failure",
  );
});

test("D1 failure without firehose binding still returns 500", async () => {
  const db = makeDb({ failInserts: true, failureMessage: "some transient error" });
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db }, ctx);
  assert.equal(response.status, 500);
});

test("missing firehose binding degrades gracefully", async () => {
  const db = makeDb();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, true);
  assert.equal(json.firehose, false);
});

test("health endpoint reports database size vs soft limit", async () => {
  const db = makeDb({ sizeAfter: 12345678 });
  const ctx = makeCtx();

  const response = await worker.fetch(new Request(HEALTH_URL, { method: "GET" }), { DB: db }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.db_size_bytes, 12345678);
  assert.equal(typeof json.db_soft_limit_bytes, "number");
  assert.equal(json.over_soft_limit, false);
});

test("unknown event type is rejected", async () => {
  const db = makeDb();
  const ctx = makeCtx();
  const response = await worker.fetch(
    postRequest(makeBody({ event: "mystery" })),
    { DB: db },
    ctx,
  );
  assert.equal(response.status, 400);
});

// ---------------------------------------------------------------------------
// Website analytics events (web_pageview / web_cta_click)
// ---------------------------------------------------------------------------

function makeWebBody(overrides = {}) {
  return {
    event: "web_pageview",
    visitor_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    path: "/pricing",
    referrer: "https://news.ycombinator.com/",
    utm_source: "hn",
    utm_medium: "social",
    utm_campaign: "launch",
    event_id: "web-event-1",
    ...overrides,
  };
}

test("web_pageview is normalized and stored in events + web_details", async () => {
  const db = makeDb();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeWebBody()), { DB: db }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, true);

  const eventsInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(eventsInsert, "events row inserted");
  // visitor_id doubles as the telemetry id; version/os/arch are defaulted.
  assert.ok(eventsInsert.values.includes("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
  assert.ok(eventsInsert.values.includes("web"));

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert, "web_details row inserted");
  assert.ok(detailInsert.values.includes("/pricing"));
  assert.ok(detailInsert.values.includes("hn"));
});

test("web_pageview without event_id mints one so web_details still lands", async () => {
  // The real beacon (jcode-website public/beacon.js) does not send event_id;
  // web_details joins on it, so the worker must mint one server-side.
  const db = makeDb();
  const ctx = makeCtx();

  const body = makeWebBody();
  delete body.event_id;
  const response = await worker.fetch(postRequest(body), { DB: db }, ctx);
  assert.equal(response.status, 200);

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert, "web_details row inserted despite missing event_id");
  assert.ok(detailInsert.values.includes("/pricing"));
});

test("web_pageview without visitor_id is rejected", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeWebBody({ visitor_id: undefined })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 400);
});

test("web_pageview without path is rejected", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeWebBody({ path: undefined })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 400);
});

test("web_cta_click requires cta", async () => {
  const db = makeDb();
  const missing = await worker.fetch(
    postRequest(makeWebBody({ event: "web_cta_click" })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(missing.status, 400);

  const ok = await worker.fetch(
    postRequest(makeWebBody({ event: "web_cta_click", cta: "plus_early_access" })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(ok.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert.values.includes("plus_early_access"));
});

test("web free-text fields are length-capped (size defense)", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeWebBody({ path: "/" + "x".repeat(5000), referrer: "r".repeat(5000) })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  for (const value of detailInsert.values) {
    assert.ok(String(value).length <= 200, "web detail values capped at 200 chars");
  }
});

test("web events are firehosed to FIREHOSE_WEB with visitor_id as index1", async () => {
  const db = makeDb();
  const firehose = makeFirehose();
  const webFirehose = makeFirehose();

  const response = await worker.fetch(
    postRequest(makeWebBody({ event: "web_cta_click", cta: "install" })),
    { DB: db, FIREHOSE: firehose, FIREHOSE_WEB: webFirehose },
    makeCtx(),
  );
  const json = await response.json();

  assert.equal(json.firehose, true);
  assert.equal(firehose.points.length, 0, "CLI firehose untouched by web events");
  assert.equal(webFirehose.points.length, 1);
  const point = webFirehose.points[0];
  assert.deepEqual(point.indexes, ["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]);
  // FIREHOSE_WEB_SCHEMA blob positions (append-only contract):
  assert.equal(point.blobs[0], "web_cta_click"); // blob1 = event
  assert.equal(point.blobs[7], "/pricing"); // blob8 = path
  assert.equal(point.blobs[9], "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"); // blob10 = visitor_id
  assert.equal(point.blobs[13], "install"); // blob14 = cta
});

// ---------------------------------------------------------------------------
// Token subscription plan events
// ---------------------------------------------------------------------------

function makeSubscriptionBody(overrides = {}) {
  return makeBody({
    event: "subscription_activated",
    step: undefined,
    auth_provider: undefined,
    auth_method: undefined,
    auth_failure_reason: undefined,
    account_id: "acct_123",
    tier: "plus",
    ...overrides,
  });
}

test("subscription events require account_id", async () => {
  const db = makeDb();
  for (const event of [
    "subscription_login",
    "subscription_activated",
    "subscription_budget_exhausted",
    "subscription_router_error",
    "account_linked",
  ]) {
    const response = await worker.fetch(
      postRequest(makeSubscriptionBody({ event, account_id: undefined })),
      { DB: db },
      makeCtx(),
    );
    assert.equal(response.status, 400, `${event} without account_id rejected`);
  }
});

test("subscription_activated stores account_id and tier", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeSubscriptionBody()),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert.sql.includes("account_id"));
  assert.ok(insert.sql.includes("tier"));
  assert.ok(insert.values.includes("acct_123"));
  assert.ok(insert.values.includes("plus"));
});

test("subscription model is stored in the generic model_start column", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeSubscriptionBody({ event: "subscription_router_error", model: "gpt-5.5" })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert.sql.includes("model_start"));
  assert.ok(insert.values.includes("gpt-5.5"));
});

test("account_linked joins telemetry_id and account_id", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeSubscriptionBody({ event: "account_linked", tier: undefined })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert.values.includes("11111111-2222-3333-4444-555555555555"));
  assert.ok(insert.values.includes("acct_123"));
});

// ---------------------------------------------------------------------------
// CORS for the website beacon
// ---------------------------------------------------------------------------

test("OPTIONS preflight from jcode.sh echoes the origin", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://jcode.sh" },
    }),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://jcode.sh");
  assert.equal(response.headers.get("Vary"), "Origin");
  assert.ok(/POST/.test(response.headers.get("Access-Control-Allow-Methods")));
});

test("OPTIONS preflight from pages.dev preview echoes the origin", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://solosystems.pages.dev" },
    }),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://solosystems.pages.dev");
});

test("other origins fall back to ALLOWED_ORIGIN default", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://evil.example" },
    }),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "*");
});

test("POST responses from the beacon origin carry CORS headers", async () => {
  const db = makeDb();
  const request = new Request(EVENT_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Origin: "https://jcode.sh",
    },
    body: JSON.stringify(makeWebBody()),
  });
  const response = await worker.fetch(request, { DB: db }, makeCtx());
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://jcode.sh");
});
