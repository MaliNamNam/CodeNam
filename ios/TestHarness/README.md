# iOS E2E Test Harness

A deterministic, no-LLM harness for developing and validating the jcode iOS
client (`JCodeMobile`) end-to-end. It replaces the role of the old Rust
simulator: one source of honest, repeatable server behavior the client can be
built against on this machine, without a device, network, or provider cost.

## Pieces

- **`mock_gateway.py`** - a self-contained (stdlib-only) mock of the jcode
  server gateway. Speaks the exact wire protocol from
  `crates/jcode-base/src/gateway.rs` on one TCP port:
  - `GET /health` -> status/version
  - `POST /pair` -> token exchange (code `123456` by default)
  - `GET /ws` -> WebSocket upgrade carrying the newline-delimited JSON protocol
  A `message` request triggers a scripted assistant turn (reasoning, text
  deltas, a `bash` tool-call lifecycle, tokens, done). `--push-demo` also pushes
  an out-of-band notification + compaction notice after connect.

- **`protocol_smoke_test.py`** - a stdlib WebSocket/HTTP client that drives the
  mock and asserts the full happy-path event sequence (pair, subscribe,
  history, message stream, set_model). Run it against either the mock or a real
  `jcode` gateway.

- **`run_e2e.sh`** - the one-command pipeline: `swift test` -> build app ->
  start mock -> smoke test -> boot simulator -> seed a paired credential ->
  launch -> screenshot.

## Usage

```bash
# Full pipeline, screenshot lands in $TMPDIR/jcode-ios-e2e/chat.png
./TestHarness/run_e2e.sh

# Also exercise the out-of-band notice toasts
./TestHarness/run_e2e.sh --push-demo

# Just the protocol assertions against a running gateway (mock or real)
python3 TestHarness/mock_gateway.py &        # or run a real `jcode` gateway
python3 TestHarness/protocol_smoke_test.py --port 7643
```

## How auto-connect is seeded

The app stores paired servers in the Keychain, falling back to
`Library/Application Support/jcode-servers.json` when the Keychain is
unavailable (unsigned simulator builds). The harness writes that JSON directly
into the app's data container so the app auto-connects on launch, bypassing the
SpringBoard "Open in app?" deep-link confirmation that can't be scripted.

## Why this exists

`JCodeKit` (the platform-free client core) is fully unit-tested with `swift
test`. This harness adds the layer above that: it proves the real SwiftUI app,
running in a simulator, connects over a real WebSocket and renders a real
transcript. Together they make client behavior hill-climbable without a device.
