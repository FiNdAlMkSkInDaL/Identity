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

## First Implementation

The first code lives in [crates/sovereignd](crates/sovereignd). It is the local daemon core responsible for creating the Sovereign workspace, writing captured text into the SQLite transit buffer, cleaning/promoting local captures, and storing prototype `.me` memory vectors locally.

```powershell
cargo run -p sovereignd -- init
cargo run -p sovereignd -- ingest --source manual --content "User prefers local-first systems."
cargo run -p sovereignd -- list
cargo run -p sovereignd -- stats
cargo run -p sovereignd -- repair-transit
cargo run -p sovereignd -- process-once --limit 10
cargo run -p sovereignd -- process-idle-once --limit 10 --idle-ms 5000
cargo run -p sovereignd -- cleaned-list --limit 10
cargo run -p sovereignd -- promote-once --limit 10
cargo run -p sovereignd -- memory-list --limit 10
cargo run -p sovereignd -- memory-search --query "local-first systems"
cargo run -p sovereignd -- slice-preview --intent "draft outreach using local context"
cargo run -p sovereignd -- prompt-package --intent "draft outreach using local context" --prompt "Write the message."
cargo run -p sovereignd -- serve
cargo run -p sovereignd -- watch --path C:\Users\finph\Documents
```

The initial local workspace is created at:

```text
~/.sovereign/
  identity.me/
    state.db
  transit.db
  logs/
```

The first local capture endpoint listens on `127.0.0.1:8080`:

```powershell
Invoke-RestMethod -Method Get -Uri http://127.0.0.1:8080/health
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8080/capture -ContentType "text/html" -Body "<html><body><h1>Hello Sovereign</h1><script>ignore()</script></body></html>"
```
