# Identity System Map

This is the living ontology for the Identity system.

It must be updated whenever a coding pass adds, removes, or materially changes a module, data store, pipeline stage, state transition, external boundary, or command.

## 1. Current Runtime Map

```mermaid
flowchart TD
    User["User / Local Operator"]
    CLI["identityd CLI"]
    Workspace["Workspace Manager\nworkspace.rs"]
    Transit["SQLite Transit Buffer\ntransit.rs"]
    Cleaned["Cleaned Event Staging\ncleaned_events"]
    EmbedProto["Local Embedding Prototype\nembedding.rs"]
    Tokenizer["Local WordPiece Tokenizer\nvocab.txt -> tensors"]
    Memory["Identity Memory Store\nidentity.rs"]
    MemoryGraph["Prototype Memory Graph Edges\ngraph_edges"]
    VectorStore["Vector Blob Store\nLanceDB default\nfilesystem mirror fallback"]
    MemoryMeta["Memory Metadata\nstore_metadata"]
    Slice[".meslice Preview Generator\nslice.rs"]
    Proxy["Local Capture Endpoint + lol-html Cleaner\nproxy.rs"]
    FS["Filesystem Watcher\nfilesystem.rs"]
    Safety["Ingest Safety Filter\ningest_safety.rs"]
    Crypto["Local Content Protection\ncrypto.rs"]
    Processor["Transit Processor\nprocessor.rs"]
    Idle["Idle Telemetry Gate\nidle.rs"]
    Redaction["Transit Content Redaction\npost-promotion minimization"]
    Resources["Resource Budget Probe\nresource.rs"]
    LocalRoot["~/.identity/"]
    IdentityDir["identity.me/"]
    TransitDb["transit.db"]
    CaptureToken["capture.token"]
    Logs["logs/"]

    User --> CLI
    CLI --> Workspace
    Workspace --> LocalRoot
    LocalRoot --> IdentityDir
    LocalRoot --> TransitDb
    LocalRoot --> CaptureToken
    LocalRoot --> Logs

    CLI -->|"init"| Workspace
    CLI -->|"ingest"| Transit
    CLI -->|"capture-active-window"| Safety
    CLI -->|"watch-active-window"| Safety
    CLI -->|"list / stats / capture-sources / doctor / repair-transit"| Transit
    CLI -->|"doctor"| Resources
    CLI -->|"protect-at-rest"| Crypto
    CLI -->|"redact-transit-content"| Redaction
    CLI -->|"cleaned-list"| Cleaned
    CLI -->|"memory-list"| Memory
    CLI -->|"memory-stats / memory-export / memory-protocol-health / repair-protocol-schema / repair-memory-vectors"| Memory
    CLI -->|"embedding-runtime-health / embedding-active-health / onnx-runtime-health / embedding-tokenizer-health / embedding-tokenize / embedding-onnx-run / embedding-manifest-write / embedding-bootstrap"| EmbedProto
    CLI -->|"memory-search"| Memory
    CLI -->|"memory-edge-* / memory-graph-health"| MemoryGraph
    CLI -->|"slice-preview / prompt-package"| Slice
    CLI -->|"serve"| Proxy
    CLI -->|"watch"| FS
    CLI -->|"daemon"| Proxy
    CLI -->|"daemon"| Processor
    CLI -->|"daemon --watch-path"| FS
    CLI -->|"daemon --watch-active-window"| Safety
    CLI -->|"process-once"| Processor
    CLI -->|"process-idle-once"| Idle
    CLI -->|"pipeline-once / pipeline-loop"| Processor
    CLI -->|"promote-once"| Processor

    CaptureToken -->|"X-Identity-Capture-Token"| Proxy
    Proxy -->|"authorized POST /capture\ncleaned HTML/text"| Safety
    FS -->|"approved text/code file captures"| Safety
    Safety -->|"non-sensitive captures only"| Crypto
    Crypto -->|"protected capture text\n+ source labels"| Transit
    Processor -->|"claim queued"| Transit
    Idle -->|"allows processing only after idle threshold"| Processor
    Processor -->|"Unicode NFKC normalize + strip control chars\n+ mark processed"| Cleaned
    Processor -->|"mark failed"| Transit
    Processor -->|"decrypt local cleaned text\nthen embed"| EmbedProto
    EmbedProto -->|"WordPiece token tensors"| Tokenizer
    Tokenizer -->|"input_ids / attention_mask / token_type_ids"| EmbedProto
    EmbedProto -->|"ONNX opt-in or hash fallback\n384-float vector blob"| Memory
    Memory -->|"similarity/manual links"| MemoryGraph
    Memory -->|"mirror vector blobs"| VectorStore
    Processor -->|"promote protected\nsemantic text"| Memory
    Processor -->|"mark promoted"| Cleaned
    Processor -->|"redact promoted transit duplicates"| Redaction
    Redaction -->|"clear captured/cleaned content after .me write"| Transit
    Redaction --> Cleaned
    Memory --> MemoryMeta
    Slice -->|"search scoped memory"| Memory
    MemoryGraph --> IdentityDir
    Transit --> TransitDb
    Cleaned --> TransitDb
    Memory --> IdentityDir
```

## 2. Intended Full-System Map

```mermaid
flowchart TD
    OS["Ambient OS Activity"]
    Files["Approved Filesystem Roots"]
    Browser["Browser / Local Web Traffic"]
    Hotkey["Global Hotkey Listener\nhotkey.rs"]
    Daemon["identityd"]
    Transit["SQLite Transit Buffer"]
    Cleaner["Local Cleaner / SLM Summarizer"]
    Embedder["Local Embedding Runtime\nONNX / ort"]
    MeGraph[".me Hybrid Vector Graph\nLanceDB"]
    Snapshot["Context Snapshot\ncontext_snapshot.rs"]
    Profile["Project Profile Matcher\nproject_profile.rs"]
    Builder["Context Builder\ncontext_builder.rs"]
    Clipboard["Clipboard Writer\nclipboard.rs"]
    Boundary["Need-to-Know Boundary Engine"]
    Meslice["Ephemeral .meslice"]
    Remote["External LLM / Agent Endpoint"]
    Watcher["Session Watcher"]
    Delta["Semantic Delta Extractor"]
    Reconcile["Graph Reconciliation + Decay"]

    OS --> Daemon
    Files --> Daemon
    Browser --> Daemon
    Hotkey --> Snapshot

    Daemon --> Transit
    Transit --> Cleaner
    Cleaner --> Embedder
    Embedder --> MeGraph

    Snapshot --> Profile
    Profile --> Builder
    Builder --> MeGraph
    Builder --> Clipboard

    Boundary --> MeGraph
    Boundary --> Meslice
    Meslice --> Remote
    Remote --> Watcher
    Watcher --> Delta
    Delta --> Reconcile
    Reconcile --> MeGraph
```

## 3. Ontology

| Entity | Type | Current Status | Code / Document | Responsibility |
| :--- | :--- | :--- | :--- | :--- |
| `identityd` | Daemon crate | Implemented | `crates/identityd` | Local ingestion and transit-buffer orchestration. |
| `Workspace Manager` | Module | Implemented | `crates/identityd/src/workspace.rs` | Creates local Identity directories and a stable local `capture.token` used to authorize loopback capture writes. |
| `Capture Adapter Health` | Module | Implemented | `crates/identityd/src/capture.rs` | Centralizes read-only Phase 1 capture-adapter health for `doctor`, reporting manual ingest, token-protected loopback capture, safe-root filesystem capture, minimal active-window capture, and the conservative aggregate capture status. |
| `SQLite Transit Buffer` | Local store | Implemented | `crates/identityd/src/transit.rs` | Stores captured text temporarily, queue status, retry counts, stale processing lease repair, claimed-state enforcement for processing completion, redaction timestamps, protected source-family health counts, transit health reporting, and rollback-only insert latency probes. |
| `Cleaned Event Staging` | Local store | Implemented | `crates/identityd/src/transit.rs` | Stores normalized text ready for future embedding; promoted rows are redacted after insertion into local memory. |
| `Transit Content Redaction` | Privacy guard | Implemented | `crates/identityd/src/transit.rs`, `crates/identityd/src/processor.rs` | Clears duplicate captured and cleaned content after successful promotion into `.me` prototype storage while preserving hashes, timestamps, and pipeline state. |
| `Resource Budget Probe` | Resource guard | Implemented | `crates/identityd/src/resource.rs`, `crates/identityd/src/main.rs` | Reports current process working-set/pagefile memory on Windows, binary size, and budget status through `doctor` without adding a measurement dependency. |
| `Ingest Safety Filter` | Privacy guard | Implemented | `crates/identityd/src/ingest_safety.rs` | Enforces a universal 1MB capture-content budget and 2048-byte source-label budget, then blocks secret-bearing paths, SSH/AWS/Azure/GPG config roots, private keys, credential markers, known secret token prefixes, card-like numbers including spaced/dashed variants, bank-routing markers, and precise-location markers before SQLite persistence. |
| `Local Content Protection` | Privacy guard | Implemented on Windows | `crates/identityd/src/crypto.rs`, `crates/identityd/src/transit.rs`, `crates/identityd/src/identity.rs`, `crates/identityd/src/main.rs` | Protects captured text, source labels, cleaned staging text, and prototype `.me` semantic text fields before SQLite persistence. On Windows this uses the local user's DPAPI boundary; legacy plaintext rows remain readable for development migration compatibility. `doctor` reports legacy plaintext field counts and `protect-at-rest` migrates them locally. Cross-platform OS-backed backends remain future hardening work. |
| `ONNX Artifact Bootstrap` | Bootstrap tool | Implemented | `crates/identityd/src/bootstrap.rs` | Downloads the all-MiniLM-L6-v2 ONNX model and WordPiece vocabulary from Hugging Face using the system `curl.exe`, writes the adjacent `.identity.json` manifest, and prints environment variable configuration guidance. Requires no additional Rust dependencies. |
| `Local Embedding Prototype` | Compute stage | Implemented prototype with explicit runtime boundary | `crates/identityd/src/embedding.rs` | Generates deterministic local 384-dimensional embeddings for promoted cleaned text, exposes model/runtime metadata, preflights a configured `IDENTITY_EMBEDDING_MODEL_PATH` for local `.onnx` artifact existence/type/size plus an adjacent `<model>.onnx.identity.json` manifest with the expected embedding dimension, can write that sidecar through `embedding-manifest-write`, can validate and run local WordPiece tokenization through `embedding-tokenizer-health` and `embedding-tokenize`, can attempt a read-only ONNX Runtime session load through `onnx-runtime-health`, can execute an explicit local ONNX embedding smoke path through `embedding-onnx-run` when built with `--features onnx-runtime`, and reports local embedding latency against the 200ms map-stage budget through `doctor`. The live `EmbeddingEngine` now selects a stable runtime/model id at store-open time: hash remains default, `IDENTITY_EMBEDDING_RUNTIME=onnx` may select the manifest model id when the local runtime is healthy, and unavailable ONNX falls back to hash. |
| `Identity Memory Store` | Local store | Implemented prototype | `crates/identityd/src/identity.rs` | Stores local `.me` memory nodes plus fixed-width vector blobs in `identity.me/state.db`; assigns each node a stable UUIDv4-style `node_uid`, UTC ISO8601 `created_at_utc`, and UTC ISO8601 `last_accessed_utc` while keeping compact SQLite row ids and millisecond timestamps for local joins and ordering; updates last-access metadata when retrieval returns a node; exports recent nodes as local protocol-shaped JSON using `node_uid` and target edge `node_uid` values rather than internal row ids; validates protocol-facing UUIDs, UTC timestamps, object-shaped structured attributes, and active vector dimensions through `doctor` and `memory-protocol-health`; repairs bounded protocol-field drift through `repair-protocol-schema`; now separates memory-domain derivation from the concrete SQLite persistence backend and routes vector encode/decode/similarity decisions through a local embedding-engine boundary to keep the promotion and retrieval surface stable ahead of later ONNX and vector-store swaps; persists the selected embedding model id/runtime separately from structural schema migration, lets empty stores adopt the active runtime, and keeps non-empty hash-backed stores on hash until an explicit re-embedding migration exists; mirrors promoted vector blobs into the reserved local vector-store root, checks the primary mirror directly instead of masking misses through SQLite fallback reads, backfills that mirror from valid SQLite vectors on open, and can fall back to SQLite when primary vector blobs are missing or corrupt during retrieval; classifies promoted captures by source type such as filesystem, local web capture, and Windows UI activity; derives structured summaries and lightweight JSON attributes for Windows activity captures from application, window, focus, and visible-text fields; searches with vector similarity plus lexical scoring; reports and repairs vector health; stores prototype graph edges between memory nodes. |
| `Prototype Memory Graph Edges` | Local store table | Implemented prototype | `crates/identityd/src/identity.rs` | Stores bounded weighted edges in `graph_edges`, rejects invalid/self edges, uses SQLite foreign-key checks, auto-links nearby vectors during memory promotion, exposes manual edge commands, reports graph health, and applies explicit edge-weight decay. This is local graph scaffolding inside the SQLite `.me` prototype, not final LanceDB graph completion. |
| `Vector Blob Store` | Local store | Implemented default | `crates/identityd/src/vector_store.rs` | Persists fixed-width vector blobs under `identity.me/vectors`, writes local store metadata, exposes primary-only reads for mirror health, and serves as the default implementation built using LanceDB. Retrieval falls through to a SQLite-backed implementation that reads inline vector blobs from `identity.me/state.db` on fallback. The LanceDB implementation is integrated as the default build option. |
| `Memory Metadata` | Local store table | Implemented prototype | `crates/identityd/src/identity.rs` | Persists current embedding model id and embedding dimension for local `.me` schema inspection. |
| `.meslice Preview Generator` | Privacy boundary | Implemented prototype | `crates/identityd/src/slice.rs` | Builds scoped context blocks and prompt packages from local memory search. |
| `Local Capture Endpoint` | Ingestion adapter | Implemented | `crates/identityd/src/proxy.rs`, `crates/identityd/src/main.rs` | Accepts local HTTP captures at `127.0.0.1:8080` only when `X-Identity-Capture-Token` matches the workspace-local `capture.token`; caps headers at 16KB and uses the shared 1MB ingest safety capture budget; accepts only textual content types; non-loopback binds are rejected unless explicitly forced for local development. |
| `HTML/Text Cleaner` | Capture normalizer | Implemented | `crates/identityd/src/proxy.rs`, `lol-html` | Extracts visible document text through a lightweight streaming parser and ignores script/style raw text before transit persistence. |
| `Active Window Capture` | Ingestion adapter | Implemented cross-platform | `crates/identityd/src/activity.rs`, `crates/identityd/src/main.rs` | Captures the current foreground window title, executable name, focused-control text, and a bounded set of visible child-window text strings on Windows, macOS, and Linux. Extracts focused-control text via UI Automation/MSAA on Windows, AppleScript on macOS, and X11 utility queries on Linux. One-shot and bounded watch modes are both available. |
| `Filesystem Watcher` | Ingestion adapter | Implemented | `crates/identityd/src/filesystem.rs` | Uses `ReadDirectoryChangesW` on Windows, falls back to polling with `--poll`, validates watch roots before startup, refuses broad/sensitive roots unless `--allow-unsafe-watch-root` is explicitly passed, exposes this policy through `doctor`, filters text-like files by extension (25 supported extensions including code, config, and data formats), checks first 512 bytes for null bytes to skip binary files with text extensions, treats invalid UTF-8 as non-text, retries transient Windows file locks, reads up to 1MB per file, and dedupes burst events by per-path content hash. |
| `Idle Telemetry Gate` | Resource guard | Implemented minimal | `crates/identityd/src/idle.rs` | Gates processing by recent user input on Windows (`GetLastInputInfo`) and falls open where OS telemetry is unavailable. |
| `Transit Processor` | Pipeline worker | Implemented | `crates/identityd/src/processor.rs` | Claims queued captures, stages cleaned output through Unicode NFKC normalization with control-character stripping, promotes cleaned rows into local memory, and runs idle-gated pipeline cycles. The daemon pipeline loop is resilient to transient errors: it logs and retries rather than crashing. |
| `Context Snapshot` | Context capture module | Implemented (Phase 2) | `crates/identityd/src/context_snapshot.rs` | Reads active foreground window metadata (process name, title, focused-control text, optional selected text) on demand without queuing a transit capture. Reuses existing `activity.rs` native Windows calls. |
| `Project Profile Matcher` | Deterministic classifier | Implemented (Phase 2) | `crates/identityd/src/project_profile.rs`, `~/.identity/projects.json` | Loads JSON/TOML project profiles, matches the active context snapshot against window title, process name, and path substrings, and returns project guardrails and memory query terms. No ML required. |
| `Context Builder` | Context formatter | Implemented (Phase 2) | `crates/identityd/src/context_builder.rs` | Combines context snapshot, matched project profile, and memory search results into a structured, sanitized `[IDENTITY CONTEXT]` block. Enforces char/token budget (default 8000 chars). Strips internal IDs, hashes, and scores. Sanitizes focused text to prevent prompt injection. |
| `Clipboard Writer` | Output adapter | Implemented (Phase 2) | `crates/identityd/src/clipboard.rs` | Writes a UTF-16 string to the Windows clipboard using native `OpenClipboard`/`SetClipboardData`/`CloseClipboard` API calls. No GUI framework dependency. |
| `Hotkey Listener` | Input adapter | Implemented (Phase 2) | `crates/identityd/src/hotkey.rs` | Registers a global system hotkey via Win32 `RegisterHotKey`/`GetMessage`, fires the context injection pipeline on press, debounces rapid repeats, and runs only when `--hotkey` flag is passed. Does not block the daemon ingestion pipeline. |
| `Tauri Overlay` | UI shell | Deferred | `docs/engineering-roadmap.md` | Optional future visual overlay. Not required for V0 clipboard-first hotkey injection. |
| `.me Vector Graph` | Durable local state | Implemented LanceDB | `crates/identityd/src/identity.rs`, `docs/local-vector-synthesis-architecture.md` | Durable local hybrid graph fully integrated with embedded LanceDB vector store by default. |
| `Local Embedding Runtime` | Compute stage | Opt-in ONNX attempt / default hash fallback | `crates/identityd/src/embedding.rs`, `docs/local-vector-synthesis-architecture.md` | Current default implementation is deterministic local hashing. The optional `onnx-runtime` Cargo feature adds `ort` with default features disabled and dynamic loading, validates low-thread ONNX session loading, and can run an explicit local embedding inference smoke path from WordPiece tensors to a normalized 384-dimensional vector. `IDENTITY_EMBEDDING_RUNTIME=onnx` lets the live engine attempt ONNX during promotion/search while preserving hash fallback for local pipeline continuity. |
| `Boundary Engine` | Privacy gate | Planned | `docs/ephemeral-handshake-architecture.md` | Chooses minimum context needed for a task. |
| `.meslice` | Ephemeral payload | Planned | `docs/ephemeral-handshake-architecture.md` | Task-bound context stream for external agents. |
| `Session Watcher` | Feedback observer | Planned | `docs/bidirectional-state-synchronization-architecture.md` | Captures scoped outputs from agent sessions. |
| `Semantic Delta Extractor` | Feedback processor | Planned | `docs/bidirectional-state-synchronization-architecture.md` | Converts session logs into structured state deltas. |
| `Graph Reconciliation` | State merger | Planned | `docs/bidirectional-state-synchronization-architecture.md` | Merges deltas and decays outdated edges. |

## 4. Local Workspace Ontology

```text
~/.identity/
  identity.me/   implemented prototype memory store directory
    state.db     implemented SQLite `.me` staging ledger with vector blobs
            memory_nodes
                node_uid               UUIDv4-style protocol-facing memory id
                created_at_utc         UTC ISO8601 protocol creation timestamp
                last_accessed_utc      UTC ISO8601 protocol retrieval timestamp
                structured_attributes  lightweight JSON capture facets for direct local lookup
      store_metadata
      graph_edges
        vectors/     default filesystem-backed vector blob store and reserved future embedded vector DB root
            store.meta
            node-*.f32le
            lancedb/  optional when built with the `lancedb-backend` feature
  transit.db     implemented SQLite transit buffer and cleaned staging
    captured_events.content_redacted_at_ms
    cleaned_events.content_redacted_at_ms
  capture.token  local loopback capture write token
  logs/          reserved local daemon logs
```

## 5. Local Workspace Additions (Phase 2)

```text
~/.identity/
  projects.json   optional deterministic project profile config
                  matches window title / process / path to project id
                  contains per-project memory query terms and guardrails
```

## 6. Implemented Command Surface

Global `--root <folder>` can be used before a command to run against an explicit Identity workspace root, which keeps tests and development runs out of the real `~/.identity` ledger.

| Command | Pipeline | Inputs | Writes | Current Purpose |
| :--- | :--- | :--- | :--- | :--- |
| `init` | Workspace bootstrap | None | `~/.identity/*`, `capture.token` | Creates local workspace, capture token, and transit DB. |
| `ingest` | Manual capture | `--source`, `--content` | `captured_events` | Queues a text event manually. |
| `capture-active-window` | Windows activity capture | None | `captured_events` | Captures the current foreground window title, application name, focused-control text, and bounded visible UI text into the local transit buffer on Windows. |
| `watch-active-window` | Windows activity watch | `--interval-ms` | `captured_events` | Polls the foreground window at a bounded interval and queues captures only when the application or title changes. |
| `list` | Inspection | None | None | Lists recent captured events. |
| `stats` | Inspection | None | None | Counts events by status. |
| `capture-sources` | Inspection | None | None | Prints protected capture source-family counts for manual, loopback, filesystem, active-window, and other local ingress without exposing raw paths or source labels. |
| `doctor` | Phase 1 health inspection | `--lease-ms` | Rollback-only SQLite probe, embedding latency probe, resource budget probe | Prints workspace paths, transit health, stale processing count, protected capture source-family counts, memory vector health, primary vector mirror health, memory `node_uid`, creation timestamp, last-access timestamp, protocol export schema health, embedding model/runtime/artifact/manifest metadata, ONNX Runtime session health, tokenizer vocabulary health, centralized capture adapter health, filesystem watch-root policy metadata, workspace startup readiness timing, local transit insert latency budget status, embedding map-stage latency budget status, process memory budget status, binary-size budget status, content-protection backend, unprotected legacy field counts, explicit Phase 1 readiness markers including `phase1_embedding_artifact`, a `phase1_foundation_completion_percent` score, the next concrete Phase 1 milestone, and remaining blockers for final Phase 1 completion. |
| `repair-transit` | Transit repair | `--lease-ms` | `captured_events.status`, `captured_events.retry_count` | Requeues stale `processing` claims after a bounded lease timeout. |
| `protect-at-rest` | Privacy repair | `--limit` | `captured_events.source`, `captured_events.content`, `cleaned_events.source`, `cleaned_events.cleaned_content`, memory semantic text fields | Converts legacy plaintext development rows into the current protected-at-rest format without changing local API output. |
| `redact-transit-content` | Data minimization | `--limit` | `captured_events.content`, `cleaned_events.cleaned_content`, redaction timestamps | Clears duplicate content from promoted transit rows after `.me` storage succeeds. |
| `cleaned-list` | Inspection | `--limit` | None | Lists normalized text staged for vectorization. |
| `memory-list` | Inspection | `--limit` | None | Lists local identity memory nodes, including internal row ids, protocol-facing `node_uid` values, created UTC timestamps, and last-accessed UTC timestamps. |
| `memory-stats` | Inspection | None | None | Prints `.me` prototype node count, protocol id/timestamp health counts, SQLite vector health, primary vector mirror counts, embedding model id, embedding dimension, and embedding runtime metadata. |
| `embedding-runtime-health` | Inspection | None | None | Prints embedding runtime metadata plus local ONNX artifact and sidecar-manifest preflight status from `IDENTITY_EMBEDDING_MODEL_PATH` without loading a model. |
| `embedding-active-health` | Inspection | Optional `IDENTITY_EMBEDDING_RUNTIME=onnx` plus ONNX model/vocab/runtime environment | None | Prints the requested embedding runtime, active runtime, and fallback reason used by the live `EmbeddingEngine` for promotion/search. |
| `onnx-runtime-health` | Inspection | `IDENTITY_EMBEDDING_MODEL_PATH`, optional native ONNX Runtime dynamic library environment | None | Prints whether the daemon was compiled with the optional `onnx-runtime` feature, whether a runtime dynamic-library path is configured, and whether the ready `.onnx` artifact can be opened as a low-thread ONNX Runtime session. |
| `embedding-tokenizer-health` | Inspection | Optional `--vocab-path`, otherwise `IDENTITY_TOKENIZER_VOCAB_PATH` | None | Validates a local WordPiece vocabulary file size, readability, token count, and required `[PAD]`, `[UNK]`, `[CLS]`, and `[SEP]` tokens. |
| `embedding-tokenize` | Local tensor preparation | `--text`, optional `--vocab-path`, optional `--max-tokens` | None | Runs dependency-free local WordPiece tokenization and prints padded `input_ids`, `attention_mask`, and `token_type_ids` for BERT/MiniLM-style ONNX embedding models. |
| `embedding-onnx-run` | Feature-gated local embedding inference | `--text`, optional `--model-path`, optional `--vocab-path`, optional `--max-tokens`; requires `--features onnx-runtime` and configured ONNX Runtime dynamic library | None | Tokenizes local text, runs the local ONNX model with `input_ids`, `attention_mask`, and optional `token_type_ids`, extracts the first `f32` output tensor, pools it into the persisted 384-dimensional vector contract, normalizes the vector, and prints compact run metadata. |
| `embedding-manifest-write` | Local embedding artifact setup | `--model-path`, `--model-id`, optional `--force` | `<model>.onnx.identity.json` | Writes the local sidecar manifest expected by ONNX artifact preflight after validating that the artifact exists, is a non-empty file, and has an `.onnx` extension; refuses to overwrite an existing sidecar unless `--force` is passed, then prints artifact readiness fields. |
| `embedding-bootstrap` | ONNX model bootstrap | optional `--model-dir` | `model.onnx`, `vocab.txt`, `<model>.onnx.identity.json` | Downloads the all-MiniLM-L6-v2 ONNX model (~23MB) and WordPiece vocabulary from Hugging Face using the system `curl.exe`, writes the adjacent manifest, and prints environment variable guidance for enabling the ONNX embedding runtime. Skips existing files. |
| `memory-export` | Local protocol inspection | `--limit` | None | Prints recent `.me` prototype nodes as protocol-shaped JSON with `node_id`, protocol timestamps, semantic payload, vector floats, and graph edges addressed by target `node_uid`; internal SQLite row ids and cleaned-event ids are omitted. |
| `memory-protocol-health` | Local protocol inspection | None | None | Prints protocol-facing readiness counts for UUIDv4-style node ids, UTC timestamps, object-shaped structured attributes, and active vector dimensions. |
| `repair-protocol-schema` | Memory repair | `--limit` | `identity.me/state.db.memory_nodes`, `identity.me/vectors/node-*.f32le` | Repairs bounded protocol-facing drift by regenerating invalid UUIDv4-style node ids, backfilling UTC timestamps from local millisecond epochs, normalizing malformed structured attributes to `{}`, and rebuilding wrong-sized vectors from protected local raw text. |
| `repair-memory-vectors` | Memory repair | `--limit` | `identity.me/state.db.memory_nodes.vector_embedding` | Rebuilds missing or corrupt vector blobs locally from stored raw text. |
| `memory-search` | Local retrieval | `--query`, `--limit` | `identity.me/state.db.memory_nodes.last_accessed_*` | Searches memory nodes by vector similarity plus lexical overlap and marks returned nodes as accessed. |
| `memory-edge-add` | Memory graph mutation | `--source-id`, `--target-id`, `--relationship`, `--weight` | `identity.me/state.db.graph_edges` | Adds or updates a bounded weighted relationship between two persisted memory nodes. |
| `memory-edges-list` | Memory graph inspection | `--limit` | None | Lists recent prototype graph edges. |
| `memory-edge-decay` | Memory graph decay | `--limit` | `identity.me/state.db.graph_edges` | Applies the documented edge-weight decay formula to recent graph edges. |
| `memory-graph-health` | Memory graph inspection | None | None | Prints node, edge, orphan, and decayed-edge counts for the prototype graph. |
| `slice-preview` | Context boundary | `--intent`, `--limit` | None | Emits an ephemeral context block without raw memory IDs, hashes, or scores. |
| `prompt-package` | Context injection artifact | `--intent`, `--prompt`, `--limit` | None | Emits a local prompt package containing scoped context plus user task. |
| `serve` | Local proxy capture | `--addr`, `--allow-non-loopback`, `X-Identity-Capture-Token` for writes | `captured_events` | Runs `/health` and token-authorized `/capture`; defaults to loopback-only binding and returns bounded HTTP errors for invalid or oversized captures. |
| `watch` | Filesystem capture | `--path`, `--non-recursive`, `--poll`, `--allow-unsafe-watch-root` | `captured_events` | Uses Windows filesystem events by default on Windows and keeps polling as an explicit fallback. Validates the root against the safe-root policy and dedupes repeated same-content events per path. |
| `daemon` | Phase 1/2 local daemon orchestration | `--addr`, `--process-limit`, `--promote-limit`, `--idle-ms`, `--interval-ms`, optional `--watch-path`, `--watch-active-window`, `--activity-interval-ms`, `--non-recursive`, `--allow-non-loopback`, `--allow-unsafe-watch-root`, `--hotkey`, `--hotkey-combo`, `--paste-on-hotkey` | `captured_events`, `cleaned_events`, `identity.me/state.db`, optional filesystem and OS activity captures, optional clipboard writes | Runs the loopback capture endpoint and idle-gated clean/promote pipeline together in one process. Optional `--watch-path` adds a shutdown-aware filesystem watcher after safe-root validation; optional `--watch-active-window` adds bounded foreground-window capture. `--hotkey` registers a global system hotkey and copies generated context to clipboard on each press, with optional Ctrl+V pasting. On Windows the filesystem watcher keeps the native `ReadDirectoryChangesW` path while still stopping cleanly on `Ctrl+C`. |
| `context-now` | Phase 2 on-demand context generation | `--preview`, `--copy`, `--project`, `--limit`, `--max-chars`, `--include-current-window`, `--include-focused-text`, `--include-project-memory` | Clipboard (when `--copy`) | Implemented. Generates a compact sanitized `[IDENTITY CONTEXT]` block from the active window and `.me` memory, prints it (with `--preview`) or copies it to clipboard (with `--copy`). |
| `project-profile-list` | Phase 2 project profile inspection | None | None | Implemented. Lists known project profiles loaded from `~/.identity/projects.json` and reports which profile matches the current active window. |
| `process-once` | Transit processing | `--limit` | `captured_events.status`, `cleaned_events` | Claims captures and stages normalized text. |
| `process-idle-once` | Idle-gated transit processing | `--limit`, `--idle-ms` | `captured_events.status`, `cleaned_events` when idle | Runs one processing batch only after the configured idle threshold. |
| `pipeline-once` | Idle-gated local state pipeline | `--process-limit`, `--promote-limit`, `--idle-ms` | `captured_events.status`, `cleaned_events`, `identity.me/state.db`, redaction timestamps when idle | Runs one local clean/promote/redact cycle under the idle gate. |
| `pipeline-loop` | Repeating local state pipeline | `--process-limit`, `--promote-limit`, `--idle-ms`, `--interval-ms` | `captured_events.status`, `cleaned_events`, `identity.me/state.db`, redaction timestamps when idle | Repeats local clean/promote/redact cycles at a bounded interval. |
| `promote-once` | Memory promotion | `--limit` | `identity.me/state.db`, `cleaned_events.promoted_at_ms`, redaction timestamps | Promotes cleaned rows into local memory nodes, then redacts duplicate transit content. |

## 6. Transit Buffer State Machine

```mermaid
stateDiagram-v2
    [*] --> queued: ingest_text
    queued --> processing: claim_queued
    processing --> queued: repair_stale_processing
    processing --> processed: complete_processing_with_cleaned
    processing --> failed: mark_failed
    processed --> [*]
```

| State | Meaning | Written By |
| :--- | :--- | :--- |
| `queued` | Capture is stored and waiting for processing. | `ingest_text` |
| `processing` | Worker has claimed the event. | `claim_queued` |
| `processed` | Placeholder cleaner completed and cleaned staging was written in the same transaction. | `complete_processing_with_cleaned` |
| `failed` | Processing failed deterministically. | `mark_failed` |

`claim_queued` runs stale lease repair before claiming new work. Stale `processing`
rows are returned to `queued`, `retry_count` is incremented, and the row keeps an
error note recording the recovery event.

## 7. Implemented Ingestion Pipelines

### Manual Capture

```mermaid
sequenceDiagram
    participant User
    participant CLI as identityd ingest
    participant Transit as SQLite Transit Buffer

    User->>CLI: source + content
    CLI->>Transit: ingest_text(source, content)
    Transit-->>CLI: captured event id
```

### Local HTTP Capture

```mermaid
sequenceDiagram
    participant Client as Local Client
    participant Proxy as Local Capture Endpoint
    participant Cleaner as lol-html Cleaner
    participant Transit as SQLite Transit Buffer

    Client->>Proxy: POST /capture + X-Identity-Capture-Token
    Proxy->>Proxy: verify workspace capture token
    Proxy->>Proxy: enforce 16KB header / 1MB body budget
    Proxy->>Proxy: allow only textual media types
    Proxy->>Cleaner: clean_payload(content_type, body)
    Cleaner-->>Proxy: cleaned text
    Proxy->>Transit: ingest_text(local-proxy:*, cleaned) with ingest safety filter
    Transit-->>Proxy: captured event id
```

### Filesystem Capture

```mermaid
sequenceDiagram
    participant Watcher as Filesystem Watcher
    participant FS as Approved Folder
    participant Transit as SQLite Transit Buffer

    alt Windows native mode
        Watcher->>FS: ReadDirectoryChangesW event stream
        FS-->>Watcher: created / modified / renamed file path
    else Poll fallback
        Watcher->>Watcher: spawn blocking scan every 2s
        Watcher->>FS: walk approved root
        FS-->>Watcher: text-like file metadata
    end
    Watcher->>Transit: open local TransitBuffer
    Watcher->>Watcher: check conservative extension, size, content hash fingerprint
    Watcher->>Transit: ingest_text(filesystem:path, cleaned) with ingest safety filter
```

### Transit Processing

```mermaid
sequenceDiagram
    participant Worker as process-once
    participant Transit as SQLite Transit Buffer
    participant Cleaner as Placeholder Cleaner
    participant Staging as Cleaned Event Staging

    Worker->>Transit: claim_queued(limit)
    Transit-->>Worker: oldest queued events
    Worker->>Cleaner: clean_for_next_stage(content) [NFKC + control strip + whitespace collapse]
    Cleaner-->>Worker: ok / empty
    Worker->>Staging: complete_processing_with_cleaned
    Staging->>Transit: same SQLite transaction marks processed
    Worker->>Transit: mark_failed when cleaning returns empty content
```

### Idle-Gated Transit Processing

```mermaid
sequenceDiagram
    participant Worker as process-idle-once
    participant Idle as Idle Telemetry Gate
    participant Processor as Transit Processor

    Worker->>Idle: is_idle_for(idle-ms)
    alt idle threshold met
        Idle->>Processor: process_once(limit)
    else active user input
        Idle-->>Worker: skipped_idle_gate=true
    end
```

### Idle-Gated Local Pipeline

```mermaid
sequenceDiagram
    participant Worker as pipeline-once / pipeline-loop
    participant Idle as Idle Telemetry Gate
    participant Transit as SQLite Transit Buffer
    participant Staging as Cleaned Event Staging
    participant Embed as Local Embedding Prototype
    participant Memory as Identity Memory Store

    Worker->>Idle: is_idle_for(idle-ms)
    alt idle threshold met
        Worker->>Transit: claim queued captures
        Transit-->>Worker: captured events
        Worker->>Worker: NFKC normalize + strip control chars + collapse whitespace
        Worker->>Staging: write cleaned output transactionally
        Worker->>Staging: list cleaned pending promotion
        Worker->>Embed: embed cleaned text locally
        Worker->>Memory: insert memory node + vector blob
        Worker->>Staging: mark promoted
        Worker->>Transit: redact promoted captured/cleaned content
    else active user input
        Idle-->>Worker: skip cycle
    end
```

### Memory Promotion

```mermaid
sequenceDiagram
    participant Worker as promote-once
    participant Staging as Cleaned Event Staging
    participant Transit as SQLite Transit Buffer
    participant Embed as Local Embedding Prototype
    participant Memory as Identity Memory Store

    Worker->>Staging: list_cleaned_pending(limit)
    Staging-->>Worker: oldest unpromoted cleaned events
    Worker->>Embed: embed cleaned text into 384-float vector
    Embed-->>Worker: little-endian vector blob
    Worker->>Memory: insert_memory_from_cleaned with vector blob
    Memory-->>Worker: memory node id
    Worker->>Staging: mark_cleaned_promoted
    Worker->>Staging: redact cleaned_content after promotion
    Worker->>Transit: redact source captured content after promotion
```

### Local Memory Retrieval

```mermaid
sequenceDiagram
    participant User
    participant CLI as memory-search
    participant Memory as Identity Memory Store

    User->>CLI: query text
    CLI->>Memory: search(query, limit)
    Memory->>Memory: embed query + score vector similarity and lexical overlap
    Memory-->>CLI: highest scoring nodes
```

### Memory Vector Health

```mermaid
sequenceDiagram
    participant User
    participant CLI as memory-stats / repair-memory-vectors
    participant Memory as Identity Memory Store
    participant Embed as Local Embedding Prototype

    User->>CLI: inspect or repair vector layer
    CLI->>Memory: stats() or repair_vectors(limit)
    alt repair requested
        Memory->>Memory: find missing/corrupt vector blobs
        Memory->>Embed: rebuild embedding from local raw_text
        Embed-->>Memory: 384-float vector blob
        Memory->>Memory: update vector_embedding
    end
    Memory-->>CLI: vector health summary
```

### Phase 1 Doctor

```mermaid
sequenceDiagram
    participant User
    participant CLI as identityd doctor
    participant Transit as SQLite Transit Buffer
    participant Memory as Identity Memory Store

    User->>CLI: doctor
    CLI->>Transit: health(lease-ms)
    Transit-->>CLI: queue counts + stale processing count
    CLI->>Transit: rollback-only insert probe
    Transit-->>CLI: transit insert latency
    CLI->>Memory: stats()
    Memory-->>CLI: node count + vector health + embedding metadata
    CLI->>CLI: probe local embedding latency
    CLI->>CLI: measure process resources and binary size
    CLI->>CLI: derive Phase 1 readiness markers
    CLI-->>User: local Phase 1 health summary + remaining blockers
```

### Ephemeral Context Slice Preview

```mermaid
sequenceDiagram
    participant User
    participant CLI as slice-preview
    participant Slice as .meslice Preview Generator
    participant Memory as Identity Memory Store

    User->>CLI: intent text
    CLI->>Slice: generate_meslice(intent, limit)
    Slice->>Slice: deterministic security blacklist over intent and prompt
    Slice->>Memory: search(intent, limit)
    Memory-->>Slice: matching memory summaries
    Slice-->>CLI: IDENTITY-CONTEXT-BLOCK
```

### Prompt Package Preview

```mermaid
sequenceDiagram
    participant User
    participant CLI as prompt-package
    participant Slice as .meslice Preview Generator
    participant Memory as Identity Memory Store

    User->>CLI: intent + user prompt
    CLI->>Slice: build_prompt_package(intent, prompt, limit)
    Slice->>Slice: deterministic security blacklist
    Slice->>Memory: search(intent, limit)
    Memory-->>Slice: matching memory summaries
    Slice-->>CLI: SYSTEM CONTEXT + USER TASK
```

### Hotkey Context Injection (Implemented Phase 2)

```mermaid
sequenceDiagram
    participant User
    participant Hotkey as Global Hotkey Listener
    participant Snapshot as Context Snapshot
    participant Profile as Project Profile Matcher
    participant Builder as Context Builder
    participant Memory as Identity Memory Store
    participant Clip as Clipboard Writer

    User->>Hotkey: press Ctrl+Space (or configured combo)
    Hotkey->>Snapshot: capture_context_snapshot()
    Snapshot-->>Hotkey: process, title, focused_text, path_hint
    Hotkey->>Profile: match_profile(snapshot)
    Profile-->>Hotkey: project guardrails + memory_query_terms
    Hotkey->>Builder: build_context(snapshot, profile)
    Builder->>Memory: search(query_terms, limit=8)
    Memory-->>Builder: ranked memory summaries
    Builder->>Builder: format + budget trim (max 8000 chars)
    Builder->>Builder: strip IDs/hashes/scores, sanitize injection patterns
    Builder-->>Hotkey: [IDENTITY CONTEXT] block
    Hotkey->>Clip: copy_to_clipboard(block)
    Hotkey->>Hotkey: log confirmation line
```

## 8. Planned Pipeline Boundaries

| Boundary | Upstream | Downstream | Rule |
| :--- | :--- | :--- | :--- |
| Capture to transit | Proxy / filesystem / manual / active-window | Ingest safety filter, local content protection, then SQLite transit buffer | Enforce shared source/content budgets, reject deterministic sensitive paths, credential markers, known token prefixes, private keys, payment-card-like numbers, routing markers, and precise-location markers, then protect accepted text before SQLite persistence. |
| Transit to cleaned staging | SQLite transit buffer | Transit processor | Process only claimed events; successful cleaned writes and `processed` status updates share one SQLite transaction, and completion is rejected unless the capture is still in `processing`. |
| Cleaned staging to `.me` prototype | `cleaned_events` | Local embedding prototype, local content protection, then identity memory store | Decrypt locally for embedding, promote normalized text into local memory nodes with fixed-width vector blobs, protect semantic text fields at rest, then redact duplicate transit content. |
| Promoted transit to redacted transit | `captured_events`, `cleaned_events` | Transit redaction routine | Keep queue state, hashes, promotion markers, and redaction timestamps; clear duplicate content after `.me` insertion. |
| Idle gate to local state pipeline | Idle telemetry gate | Transit processor and memory promotion | Skip local synthesis/promotion while the user is active. |
| `.me` prototype to retrieval | Identity memory store | Local query caller | Return vector/lexical ranked memory nodes. |
| `.me` prototype to vector health | Identity memory store | Local CLI caller | Expose model metadata and repair missing/corrupt vector blobs locally. |
| `.me` prototype to `.meslice` preview | Identity memory store | `.meslice` generator | Export bounded declarative summaries only; no raw DB ids, hashes, sources, or scores. |
| `.meslice` preview to prompt package | `.meslice` generator | Local caller | Combine scoped context with user prompt without network transmission. |
| `.me` prototype to vectorization | `identity.me/state.db` | Local embedder | Embed memory nodes, not raw capture rows. |
| Vectorization to `.me` graph | Embedder | LanceDB graph | Write structured memories, not raw telemetry. |
| `.me` to `.meslice` | Boundary engine | External agent | Export minimum declarative facts only. |
| External execution to feedback | Agent endpoint | Session watcher | Capture only scoped task outputs. |
| Feedback to `.me` | Delta extractor | Graph reconciler | Validate deltas before merge. |
| Hotkey to clipboard | Hotkey listener | Context snapshot → project profile → context builder → clipboard writer | Read active window on demand, match project profile, search memory, format sanitized block, write to clipboard only. Never auto-submit. Never expose raw DB internals. |

## 9. Maintenance Rules

When changing the system, update this file in the same pass if any of the following change:

- A new crate, module, command, or local store is added.
- A pipeline gains or loses a stage.
- A state transition changes.
- A new external boundary appears.
- A planned component becomes implemented.
- A dependency changes the architecture or performance budget.

Keep this map factual. Mark future components as `Planned`; do not imply they are implemented before code exists.
