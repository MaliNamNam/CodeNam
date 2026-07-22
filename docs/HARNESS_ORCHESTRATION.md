# Harness Orchestration (OpenCode-inspired)

> Status: Phase A + Phase B implemented  
> Updated: 2026-07-23

Lightweight orchestration contracts ported from OpenCode’s harness model,
without adopting its JS/Effect monorepo. jcode keeps its shared-server,
cache-aware agent loop, and low-RAM multi-session design.

## Keep (jcode strengths)

- Shared server owns sessions; thin TUI clients attach
- Split system prompts + tool list locking (provider cache stability)
- Non-blocking memory injection
- Swarm plan DAG for multi-agent coordination
- Feature-gated heavy stacks (embeddings, PDF, Bedrock)

## Phase A — harness spine

### Permission ruleset (`jcode-permission`)

- Ordered rules: last match wins
- Actions: `allow` | `ask` | `deny`
- Wildcard patterns; tool → permission mapping
- Path-aware patterns from tool input (`file_path`, `command`, …)
- `external_directory` when a path is outside the session workspace

### Agent profiles

| Profile   | Mode     | Role                                      |
|-----------|----------|-------------------------------------------|
| `build`   | primary  | Full coding agent (default)               |
| `plan`    | primary  | Read-mostly planning; hard-deny write tools |
| `general` | subagent | Multi-step child work; todo denied        |
| `explore` | subagent | Read-only search/read/bash                |

### Loop safety

- Doom loop: 3 identical tool calls in one batch → error
- Max steps: empty tools + text-only final turn

### Skills

- System prompt + memory: **catalog only** (name + description)
- Full body via `skill_manage` load

### Thin `task` tool

Child session, depth limit, profile allowlist, XML result. Use **swarm** for multi-agent fleets.

## Phase B — harness completeness

### Interactive ask (default ON — OpenCode path)

```toml
[permission]
interactive_ask = true          # default true
ask_timeout_secs = 300
```

Env: `JCODE_INTERACTIVE_ASK=0` only for headless/CI.

When Ask fires (OpenCode UX):

1. Server emits `permission_request` → **one dock** (FIFO; more may wait behind it)
2. Keys: `y` once · `a` always · `n` deny (deny clears remaining pending, like OpenCode reject)
3. After answer, next pending (if any) becomes the dock automatically
4. Optional: `jcode permissions` offline fallback

### Profile switch (capability, not prompt-only)

- `/plan [goal]` → `set_agent_profile("plan")` then planning prompt (remote path)
- `/build` → `set_agent_profile("build")`
- Wire: `Request::SetAgentProfile`
- Session fields: `agent_profile`, `approved_permission_rules` (journaled)

### Config

**TOML** (`~/.jcode/config.toml`) still works.

**OpenCode-compatible JSON** (project or global) is also loaded:

| File | Where |
|------|--------|
| `CodeNam.json` / `CodeNam.jsonc` | project root, `.jcode/`, or `~/.jcode/` |
| `opencode.json` / `opencode.jsonc` | project root or `~/.config/opencode/` |

You can drop your existing `opencode.json` in the project (or rename/copy to `CodeNam.json`).

Supported keys (mapped into jcode): `model`, `default_agent`, `subagent_depth`, `steps`, `permission` (allow/ask/deny + nested patterns), `tools` (legacy booleans), `agent.build` / `agent.plan` model/steps.

Merge order: `config.toml` → JSON layers (global then project, cwd wins) → env vars.

```toml
[agents]
default_agent = "build"   # or "plan"
# max_steps = 50
# subagent_depth = 1

[permission]
interactive_ask = true
```

### Parallel multi-tool (OpenCode-style)

When the model issues **multiple tools in one message**, they settle
**concurrently** (up to 10), then the turn continues — same idea as OpenCode’s
eager FiberSet settlement. Meta tools that recurse (`batch`, `task`, `swarm`)
stay serial. Permission prechecks still run serially before the fan-out.

## Not ported (intentionally)

- Effect.ts service graph / full V2 event-sourced SQLite projector
- Plugin JS tools
- OpenCode monorepo HTTP dual SDK

## Crate map

| Piece              | Location                                      |
|--------------------|-----------------------------------------------|
| Ruleset + profiles | `crates/jcode-permission`                     |
| Agent fields       | `crates/jcode-app-core/src/agent.rs`          |
| Ask wait           | `safety.rs` + `enforce_tool_permission`       |
| Turn loop guards   | `agent/turn_loops.rs`, `turn_streaming_mpsc.rs` |
| Task tool          | `crates/jcode-app-core/src/tool/task.rs`      |
| Profile wire       | `jcode-protocol` `SetAgentProfile`            |
| Config             | `PermissionConfig`, `AgentsConfig`            |
| Skill catalog      | `prompt.rs`, `skill.rs` as_memory_entry       |

## Usage tips

1. OpenCode-like interactive safety: `[permission] interactive_ask = true`
2. Plan mode with hard denials: `/plan` (remote) or `default_agent = "plan"`
3. Return to full tools: `/build`
4. Focused subagent: `task({ subagent_type: "explore", ... })`
5. Multi-agent plans: `swarm`
