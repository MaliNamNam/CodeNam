# Sponsored discovery sponsor onboarding

This runbook is the source of truth for adding a tool sponsor to jcode's
`discover_tools` catalog. It covers product approval, catalog data, service
behavior, client validation, rollout, and rollback.

Sponsors pay for placement in a discovery category, not for recommendations.
The agent must still choose the best tool for the user's task and may choose a
non-sponsored alternative. Do not onboard a sponsor whose agreement requires
preferential recommendations, hidden placement, or weaker user safeguards.

## What usually changes

Adding a sponsor to an existing category is a discovery-service catalog change.
It should not require a jcode release.

A jcode code change is required when the sponsor needs:

- a new category in `DISCOVERY_CATEGORIES`;
- a response field the current client does not support;
- a new setup or provenance mechanism; or
- different disclosure, privacy, telemetry, or safety behavior.

The hosted catalog and discovery service are not stored in this repository.
Coordinate that change with the service owner. The client-side contract is in:

- `crates/jcode-app-core/src/tool/discover.rs`;
- `crates/jcode-base/src/sponsors.rs`;
- `crates/jcode-base/src/sponsors/provenance.rs`;
- `crates/jcode-tui/src/tui/app/sponsor_disclosure.rs`; and
- `TELEMETRY.md`.

## 1. Intake and approval

Record the following before editing the catalog:

- sponsor's legal and public product names;
- canonical tool name and URL;
- requested category;
- concise, factual product description;
- supported platforms and prerequisites;
- exact installation or connection steps;
- whether setup uses MCP and, if so, its exact command and arguments;
- permissions, credentials, network access, and data the tool receives;
- pricing, trial, account, payment, and other consequential requirements;
- support contact, technical owner, start date, and end or review date; and
- rollback contact and maximum acceptable disable time.

The discovery owner must verify that:

1. The product is real, reachable, and relevant to its category.
2. The description is factual rather than comparative or promotional.
3. Setup instructions use an official, versioned, or otherwise auditable
   distribution channel where possible.
4. No credential, API key, referral secret, user identifier, or environment
   value is embedded in catalog data.
5. The setup does not bypass jcode's confirmation requirements. Signups,
   payments, destructive operations, and other consequential actions still
   require the normal user confirmation and sponsorship disclosure.
6. The commercial agreement buys discoverability only. Editorial ranking and
   agent selection remain independent.

Reject or pause onboarding if any item cannot be verified.

## 2. Choose or add a category

Current categories are defined by `DISCOVERY_CATEGORIES` in
`crates/jcode-base/src/sponsors.rs`. Category values are lowercase slugs.

Use an existing category whenever it accurately describes the capability. To
add a category:

1. Add its slug to `DISCOVERY_CATEGORIES`.
2. Add the same value to the discovery service's category allowlist.
3. Update any public category documentation.
4. Run the client tests listed below.
5. Ship the jcode change before publishing entries that rely on the category.

Do not create a sponsor-specific category. Categories describe user needs, not
vendors.

## 3. Create the catalog entry

Use a stable, lowercase tool slug for `name`. A complete internal catalog entry
should contain enough data for both API phases:

```json
{
  "name": "example-tool",
  "category": "databases",
  "blurb": "Managed PostgreSQL with branching and connection pooling",
  "url": "https://example.com/product",
  "setup": "Run `npx -y example-tool-mcp@1.2.3`, then connect the resulting MCP server.",
  "mcp": {
    "command": "npx",
    "args": ["-y", "example-tool-mcp@1.2.3"]
  },
  "active": true
}
```

Client-visible fields are:

| Field | Required | Rules |
|-------|----------|-------|
| `name` | Yes | Stable canonical slug. It is also the sponsor key used by coarse MCP usage metering. |
| `blurb` | Yes | Short, factual capability description. Do not claim it is "best" or imply endorsement. |
| `url` | Recommended | HTTPS product or setup page controlled by the vendor. |
| `setup` | Select phase | Complete instructions returned only after the agent selects the tool. Never include secrets. |
| `mcp.command` | For MCP provenance | Executable used by `mcp connect`. Must exactly match the eventual connection command. |
| `mcp.args` | For MCP provenance | Ordered string array. Must exactly match the eventual connection arguments. |

The service may keep operational fields such as `category`, `active`, campaign
dates, or ordering metadata, but it must not expose private commercial data to
the client.

### Setup safety review

Run the setup in a clean test environment before publishing it. Review the
package owner, source repository, install scripts, transitive behavior, required
permissions, and credential flow. Prefer a pinned version in catalog setup
instructions. If an intentionally floating version is used, document who owns
continuous monitoring and emergency disablement.

For MCP entries, `mcp.command` and `mcp.args` are security- and
measurement-sensitive. jcode records discovery provenance only when a later MCP
connection exactly matches both values. A prose `setup` string alone does not
enable provenance tagging or coarse usage metering.

## 4. Implement the two API phases

The default client sends `GET https://api.jcode.sh/v1/discovery` with a
three-second timeout and a 64 KiB maximum response. It sends a
`User-Agent: jcode/<version>` header and a random
`x-jcode-discovery-request-id` correlation header.

The request query parameters are:

| Parameter | Phase | Meaning |
|-----------|-------|---------|
| `category` | Both | Required category slug. |
| `q` | Both | Optional short capability query. |
| `reason` | Both | Agent-stated need or selection rationale. Store and handle it as potentially sensitive text even though the client instructs the agent not to include private data. |
| `tool` | Select | Canonical name chosen from a previous browse response. Its presence selects the second phase. |

### Browse response

Browse returns eligible tools without setup instructions or MCP launch data:

```json
{
  "tools": [
    {
      "name": "example-tool",
      "blurb": "Managed PostgreSQL with branching and connection pooling",
      "url": "https://example.com/product"
    }
  ]
}
```

Return `{"tools": []}` when no entry is eligible. Do not return `setup` or
`mcp` during browse. The two-phase design requires a specific selection and
reason before setup is revealed.

### Select response

Select looks up `tool` within `category` and returns one canonical entry:

```json
{
  "tool": {
    "name": "example-tool",
    "blurb": "Managed PostgreSQL with branching and connection pooling",
    "url": "https://example.com/product",
    "setup": "Run `npx -y example-tool-mcp@1.2.3`, then connect the resulting MCP server.",
    "mcp": {
      "command": "npx",
      "args": ["-y", "example-tool-mcp@1.2.3"]
    }
  }
}
```

Use a non-2xx response for an unknown category, unknown tool, invalid request,
or service failure. Never silently substitute another sponsor. Keep total JSON
below 64 KiB and avoid redirects because they make behavior harder to audit.

## 5. Validate before production

First validate the service directly. Use generic test text because `q` and
`reason` are sent to and may be stored by the discovery service.

```bash
DISCOVERY_URL=https://staging.example.com/v1/discovery

curl --fail-with-body --get "$DISCOVERY_URL" \
  --data-urlencode 'category=databases' \
  --data-urlencode 'q=managed postgres for a test application' \
  --data-urlencode 'reason=validate the databases discovery listing'

curl --fail-with-body --get "$DISCOVERY_URL" \
  --data-urlencode 'category=databases' \
  --data-urlencode 'tool=example-tool' \
  --data-urlencode 'reason=selected for staging validation after reviewing the listed database options'
```

Verify all of the following:

- browse includes the sponsor exactly once in the intended category;
- browse omits `setup`, `mcp`, credentials, and private campaign metadata;
- select returns the same canonical `name`, plus reviewed setup instructions;
- an unknown tool and category fail rather than returning a default entry;
- response bodies remain under 64 KiB;
- logged query and reason text follows the service's retention and access policy;
- the request ID appears in service logs and can be correlated for reliability
  debugging without a persistent user identifier; and
- disabling the catalog entry removes it from browse and prevents selection.

Then validate through jcode by pointing a test config at staging:

```toml
[sponsors]
enabled = true
endpoint = "https://staging.example.com/v1/discovery"
```

In a disposable jcode session, browse the category, select the sponsor, and, if
applicable, connect the advertised MCP server. Confirm:

- the first discovery use displays `(sponsored discovery)`;
- the browse output says placement does not imply preference;
- setup appears only after selection;
- consequential next actions still request confirmation and mention the
  sponsorship;
- an MCP connection using the exact structured command and arguments displays
  discovery provenance; and
- the tool works without requiring undisclosed permissions or data.

For client-side changes, run at minimum:

```bash
cargo test -p jcode-app-core tool::discover
cargo test -p jcode-base sponsors
cargo test -p jcode-base discovery_provenance
cargo test -p jcode-tui sponsor_disclosure
cargo check -p jcode
```

If Cargo's filters change, run the containing crate's tests instead of skipping
the check.

## 6. Roll out and monitor

1. Publish to staging and complete the validation checklist.
2. Obtain sign-off from the discovery owner and the setup security reviewer.
3. Publish the entry disabled or outside its campaign window, if supported.
4. Enable it in production without changing unrelated catalog entries.
5. Repeat one browse and one select request against production.
6. Monitor discovery success/failure rates, response size and latency, browse to
   select behavior, and coarse provenance usage. See `TELEMETRY.md` for the
   client telemetry boundary.
7. Re-review setup and destination URLs whenever the vendor changes its package,
   ownership, permissions, or authentication flow.

Do not use selection or usage counts to make the agent prefer a sponsor. Those
signals are for reliability, aggregate reporting, and catalog quality.

## 7. Roll back

The primary rollback is to disable or remove the sponsor's service-side catalog
entry. This must stop both browse placement and direct select lookup. Use it for
security concerns, misleading copy, broken setup, expired agreements, service
abuse, or vendor request.

After disabling:

1. Verify browse no longer returns the entry.
2. Verify direct selection of its name fails.
3. Preserve only the logs and aggregate records required by the applicable
   retention policy.
4. Notify the technical and commercial owners.
5. Open a post-incident issue if users could have installed unsafe or incorrect
   software.

A client release is necessary only if catalog disablement cannot contain the
problem, for example a compromised category-wide response or a flaw in client
setup handling.

## Definition of done

A sponsor is onboarded only when every box is checked:

- [ ] Intake, ownership, campaign dates, and rollback contact are recorded.
- [ ] Placement-only policy and independent recommendations are accepted.
- [ ] Category and factual copy are approved.
- [ ] Setup and destination URL pass security review in a clean environment.
- [ ] Browse and select responses match the documented schemas.
- [ ] Browse does not expose setup or MCP launch data.
- [ ] MCP command and arguments exactly match the tested connection, if used.
- [ ] Staging jcode validation passes, including disclosure and confirmation.
- [ ] Unknown and disabled entries fail closed.
- [ ] Production smoke tests pass and monitoring has an owner.
- [ ] Rollback has been tested or demonstrated by disabling the staging entry.
