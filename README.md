# Identity

Identity is the home of the `.me` protocol: a decentralized, local-first context-state standard for the agentic web.

The core idea is simple: instead of platforms owning a user's identity, preferences, memory, network graph, and operational context, the user keeps that state in an encrypted local `.me` file. AI agents can request narrowly scoped, ephemeral context from that local state bank, complete a task, and retain nothing afterward.

## Product Thesis

The web is moving away from human-operated graphical interfaces and toward abstract AI agents that fetch data, execute tasks, and communicate with services directly. If the same large platforms control both the agent interface and the user's context data, user sovereignty collapses into another walled garden.

The `.me` protocol creates a user-owned identity and context layer for that future.

## Core Components

- Local `.me` state bank: an encrypted, locally hosted vector knowledge graph and state ledger.
- Ambient interface: a universal hotkey, OS sidebar, terminal, or lightweight agent window for user intent.
- Context handshake: a zero-knowledge, narrowly scoped exchange that streams only task-relevant context.
- Stateless execution: external agents and APIs receive the minimum authorization and context required, then drop all session state.
- Local memory update: task results, preferences, and new facts are written back into the user's local state bank.

## Example

```text
Analyze my industry network, find three relevant founders,
and draft personalized outreach matching my writing tone from last week.
```

The `.me` client would retrieve only the relevant network graph, writing-tone profile, and task constraints, package them into a token-optimized payload, pass that payload to an agent, and then persist the final memory locally after execution.

## Design Principles

- Local sovereignty by default.
- Zero-trust cloud assumptions.
- Client-side synthesis using local models or WebGPU where possible.
- Protocol interoperability over platform lock-in.
- Headless, machine-to-machine operation rather than brittle UI automation.

## Documents

- [Coding Agent Specification](AGENTS.md)
- [Agent Constraints and Performance Budget](docs/agent-constraints-and-performance-budget.md)
- [System Map](docs/system-map.md)
- [Architecture and Product Manifesto](docs/manifesto.md)
- [Technical Challenges and Moats](docs/technical-challenges-and-moats.md)
- [Strategic Threat Vector Analysis](docs/strategic-threat-vector-analysis.md)
- [Local Vector Synthesis Architecture](docs/local-vector-synthesis-architecture.md)
- [Ephemeral Handshake Architecture](docs/ephemeral-handshake-architecture.md)
- [Bi-Directional State Synchronization Architecture](docs/bidirectional-state-synchronization-architecture.md)
- [Engineering Roadmap](docs/engineering-roadmap.md)
- [Go-To-Market Plan](docs/go-to-market-plan.md)

## First Implementation

The first code lives in [crates/identityd](crates/identityd). It is the local daemon core responsible for creating the Identity workspace, writing captured text into the SQLite transit buffer, capturing bounded foreground-window context on Windows including focused-control text, message-based control-text fallback, and a narrow accessibility fallback, cleaning/promoting local captures, and storing prototype `.me` memory vectors locally.

```powershell
cargo run -p identityd -- init
cargo run -p identityd -- --root C:\Temp\identity-dev doctor
cargo run -p identityd -- ingest --source manual --content "User prefers local-first systems."
cargo run -p identityd -- capture-active-window
cargo run -p identityd -- watch-active-window --interval-ms 1000
cargo run -p identityd -- list
cargo run -p identityd -- stats
cargo run -p identityd -- doctor
cargo run -p identityd -- repair-transit
cargo run -p identityd -- protect-at-rest --limit 100
cargo run -p identityd -- process-once --limit 10
cargo run -p identityd -- process-idle-once --limit 10 --idle-ms 5000
cargo run -p identityd -- pipeline-once --process-limit 10 --promote-limit 10 --idle-ms 5000
cargo run -p identityd -- pipeline-loop --process-limit 10 --promote-limit 10 --idle-ms 5000 --interval-ms 2000
cargo run -p identityd -- daemon --process-limit 10 --promote-limit 10 --idle-ms 5000 --interval-ms 2000
cargo run -p identityd -- daemon --watch-active-window --activity-interval-ms 1000
cargo run -p identityd -- daemon --watch-path C:\Users\finph\Documents --process-limit 10 --promote-limit 10
cargo run -p identityd -- cleaned-list --limit 10
cargo run -p identityd -- promote-once --limit 10
cargo run -p identityd -- redact-transit-content --limit 100
cargo run -p identityd -- memory-list --limit 10
cargo run -p identityd -- memory-stats
cargo run -p identityd -- repair-memory-vectors --limit 100
cargo run -p identityd -- memory-search --query "local-first systems"
cargo run -p identityd -- memory-graph-health
cargo run -p identityd -- memory-edges-list --limit 10
cargo run -p identityd -- memory-edge-add --source-id 1 --target-id 2 --relationship RELATED_TO --weight 0.8
cargo run -p identityd -- memory-edge-decay --limit 100
cargo run -p identityd -- slice-preview --intent "draft outreach using local context"
cargo run -p identityd -- prompt-package --intent "draft outreach using local context" --prompt "Write the message."
cargo run -p identityd -- serve
cargo run -p identityd -- watch --path C:\Users\finph\Documents
cargo run -p identityd -- watch --path C:\Users\finph\Documents --poll
```

`watch` uses Windows filesystem events by default on Windows. Use `--poll` only as
the conservative fallback.

The default daemon build uses the lean filesystem-backed vector blob store with
SQLite fallback. The experimental LanceDB backend is feature-gated because the
current Rust LanceDB stack requires a heavier native build toolchain, including
`protoc`:

```powershell
cargo build -p identityd --features lancedb-backend
```

Prototype `.me` memory nodes include a stable UUIDv4-style `node_uid` alongside
the compact internal SQLite row id. The row id remains the local graph join key
for now; `node_uid` is the protocol-facing identifier for future schema and
interoperability work.

The initial local workspace is created at:

```text
~/.identity/
  identity.me/
    state.db
    vectors/
  transit.db
  capture.token
  logs/
```

Captured content, source labels, cleaned staging rows, and prototype `.me`
semantic text fields are protected before SQLite persistence. On Windows the
daemon uses the local user's DPAPI boundary; existing plaintext development
rows remain readable so local migrations do not strand old test data.
Use `protect-at-rest` to convert legacy plaintext development rows into the
current protected format.

The first local capture endpoint listens on `127.0.0.1:8080`:

```powershell
Invoke-RestMethod -Method Get -Uri http://127.0.0.1:8080/health
$token = Get-Content "$env:USERPROFILE\.identity\capture.token"
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8080/capture -Headers @{"X-Identity-Capture-Token"=$token} -ContentType "text/html" -Body "<html><body><h1>Hello Identity</h1><script>ignore()</script></body></html>"
```

`/capture` is intentionally narrow: it requires the workspace-local token,
accepts headers up to 16KB, accepts capture bodies up to 1MB, and only accepts
textual media types: `text/plain`, `text/html`, `text/markdown`,
`application/json`, `application/x-ndjson`, `application/xml`, and
`application/xhtml+xml`.

All capture paths share the same transit safety gate before SQLite persistence:
capture content is capped at 1MB, source labels are capped at 2048 bytes, and
deterministic secret, credential, payment-card, routing, and precise-location
markers are rejected.

Use `doctor` as the Phase 1 readiness check. It reports raw queue/vector health
plus explicit Phase 1 markers, including the local pipeline status and the
remaining blockers before Phase 1 can be considered complete. On Windows it
also reports current process memory usage, idle-memory budget status, binary
size, and binary-size budget status without pulling in a heavy measurement
dependency. It also probes the current local embedding path against the
200ms map-stage target and reports whether any legacy plaintext fields still
need `protect-at-rest`.

`daemon` is the phase 1 convenience entrypoint. It runs the loopback capture server and the idle-gated clean/promote pipeline in one process, and it can optionally add a shutdown-aware filesystem watcher with `--watch-path` plus bounded foreground-window capture with `--watch-active-window`. On Windows the filesystem watcher stays on the native event path.
