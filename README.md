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
cargo run -p identityd -- capture-sources
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
cargo run -p identityd -- embedding-runtime-health
cargo run -p identityd -- onnx-runtime-health
cargo run -p identityd -- embedding-tokenizer-health --vocab-path C:\Models\vocab.txt
cargo run -p identityd -- embedding-tokenize --vocab-path C:\Models\vocab.txt --text "local private context"
cargo run -p identityd --features onnx-runtime -- embedding-onnx-run --model-path C:\Models\minilm.onnx --vocab-path C:\Models\vocab.txt --text "local private context"
cargo run -p identityd -- embedding-manifest-write --model-path C:\Models\minilm.onnx --model-id minilm-l6-v2-local
cargo run -p identityd -- memory-export --limit 3
cargo run -p identityd -- memory-protocol-health
cargo run -p identityd -- repair-protocol-schema --limit 100
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
the conservative fallback. Filesystem capture refuses broad or sensitive roots
such as the home directory, drive root, Identity workspace, `.ssh`, `.aws`,
`.azure`, `.gnupg`, AppData, Windows, and Program Files unless
`--allow-unsafe-watch-root` is passed for explicit local development.

The default daemon build uses the lean filesystem-backed vector blob store with
SQLite fallback. The experimental LanceDB backend is feature-gated because the
current Rust LanceDB stack requires a heavier native build toolchain, including
`protoc`:

```powershell
cargo build -p identityd --features lancedb-backend
```

The current embedding runtime is explicit but still intentionally marked as a
prototype: `embedding.rs` exposes runtime and ONNX artifact preflight metadata
through `doctor`, `memory-stats`, `embedding-runtime-health`, and
`onnx-runtime-health`, while the final ONNX/`ort` embedding path remains the
main Phase 1 runtime blocker. Set
`IDENTITY_EMBEDDING_MODEL_PATH` to preflight a local `.onnx` model artifact
without loading it. The preflight also expects a small adjacent manifest named
`<model>.onnx.identity.json` with at least the current persisted vector shape:

```json
{
  "model_id": "minilm-l6-v2-local",
  "embedding_dim": 384
}
```

Use `embedding-manifest-write --model-path <file.onnx> --model-id <id>` to
generate that sidecar locally. Existing sidecars are left untouched unless
`--force` is passed, and the command prints the same artifact readiness fields
used by `doctor`.

The real ONNX Runtime dependency is feature-gated so the default daemon remains
lean and does not download or copy native binaries. Build with
`--features onnx-runtime` and configure the native ONNX Runtime dynamic library
for `ort` to let `onnx-runtime-health` attempt a read-only session load with
single-threaded, low-optimization settings. Successful session loading proves
the local runtime boundary; model-specific tokenization and tensor extraction
are the next embedding implementation step.

The local tokenizer boundary is explicit too. `embedding-tokenizer-health`
validates a local WordPiece `vocab.txt`, either from `--vocab-path` or
`IDENTITY_TOKENIZER_VOCAB_PATH`, and `embedding-tokenize` emits padded
`input_ids`, `attention_mask`, and `token_type_ids` tensors for BERT/MiniLM-style
embedding models. This remains local-only and dependency-free; it is the tensor
preparation step that lets the next pass connect tokenized text to ONNX Runtime
session execution.

When built with `--features onnx-runtime`, `embedding-onnx-run` performs that
connection as an explicit smoke path: it tokenizes local text, feeds
`input_ids` and `attention_mask` into the ONNX session, includes `token_type_ids`
when the model declares that input, extracts the first `f32` output tensor, pools
it into the persisted 384-dimensional vector contract, and normalizes the result.
The default daemon still uses the prototype hash embedding runtime until this
path is validated with a real local model and runtime library.

Set `IDENTITY_EMBEDDING_RUNTIME=onnx` to let the live `EmbeddingEngine` attempt
the same local ONNX path during promotion and search. If ONNX is unavailable or
misconfigured, the engine falls back to the hash runtime instead of breaking the
local pipeline. `embedding-active-health` and `doctor` report the requested
runtime, active runtime, and fallback reason. For vector-family safety, an
existing non-empty hash-backed `.me` store remains hash-backed until an explicit
local re-embedding migration exists; empty stores may adopt the healthy ONNX
manifest model id.

Prototype `.me` memory nodes include a stable UUIDv4-style `node_uid`, UTC
ISO8601 `created_at_utc` and `last_accessed_utc` timestamps alongside compact
internal SQLite row ids and millisecond timestamps. The row id remains the local
graph join key for now; `node_uid` and ISO timestamps are the protocol-facing
fields for future schema and interoperability work. `memory-export` renders a
local JSON inspection view that follows the documented protocol shape without
exposing compact internal SQLite row ids; `memory-protocol-health` validates
that this protocol-facing shape is ready. `repair-protocol-schema` repairs
bounded local protocol-field drift without leaving the device.

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

Use `doctor` as the Phase 1 readiness check. It reports raw queue/vector health,
primary vector mirror health, explicit Phase 1 markers, and a
`phase1_foundation_completion_percent` score for the local-first foundation
already implemented. It also lists the remaining blockers before Phase 1 can be
considered complete. On Windows it reports current process memory usage,
idle-memory budget status, binary size, and binary-size budget status without
pulling in a heavy measurement dependency. It also probes the current local
embedding path against the 200ms map-stage target, reports the configured ONNX
artifact as its own Phase 1 readiness marker, reports filesystem watch-root
policy enforcement, reports each local capture adapter status plus protected
source-family counts, and reports whether any legacy plaintext fields still need
`protect-at-rest`.

`daemon` is the phase 1 convenience entrypoint. It runs the loopback capture server and the idle-gated clean/promote pipeline in one process, and it can optionally add a shutdown-aware filesystem watcher with `--watch-path` plus bounded foreground-window capture with `--watch-active-window`. On Windows the filesystem watcher stays on the native event path. `--watch-path` uses the same safe-root policy as `watch`.
