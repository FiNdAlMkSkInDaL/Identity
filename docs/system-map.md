# Sovereign System Map

This is the living ontology for the Sovereign system.

It must be updated whenever a coding pass adds, removes, or materially changes a module, data store, pipeline stage, state transition, external boundary, or command.

## 1. Current Runtime Map

```mermaid
flowchart TD
    User["User / Local Operator"]
    CLI["sovereignd CLI"]
    Workspace["Workspace Manager\nworkspace.rs"]
    Transit["SQLite Transit Buffer\ntransit.rs"]
    Cleaned["Cleaned Event Staging\ncleaned_events"]
    EmbedProto["Local Embedding Prototype\nembedding.rs"]
    Memory["Identity Memory Store\nidentity.rs"]
    Slice[".meslice Preview Generator\nslice.rs"]
    Proxy["Local Capture Endpoint + lol-html Cleaner\nproxy.rs"]
    FS["Filesystem Poller\nfilesystem.rs"]
    Safety["Ingest Safety Filter\ningest_safety.rs"]
    Processor["Transit Processor\nprocessor.rs"]
    Idle["Idle Telemetry Gate\nidle.rs"]
    LocalRoot["~/.sovereign/"]
    IdentityDir["identity.me/"]
    TransitDb["transit.db"]
    Logs["logs/"]

    User --> CLI
    CLI --> Workspace
    Workspace --> LocalRoot
    LocalRoot --> IdentityDir
    LocalRoot --> TransitDb
    LocalRoot --> Logs

    CLI -->|"init"| Workspace
    CLI -->|"ingest"| Transit
    CLI -->|"list / stats / repair-transit"| Transit
    CLI -->|"cleaned-list"| Cleaned
    CLI -->|"memory-list"| Memory
    CLI -->|"memory-search"| Memory
    CLI -->|"slice-preview / prompt-package"| Slice
    CLI -->|"serve"| Proxy
    CLI -->|"watch"| FS
    CLI -->|"process-once"| Processor
    CLI -->|"process-idle-once"| Idle
    CLI -->|"promote-once"| Processor

    Proxy -->|"POST /capture\ncleaned HTML/text"| Safety
    FS -->|"approved text/code file captures"| Safety
    Safety -->|"non-sensitive captures only"| Transit
    Processor -->|"claim queued"| Transit
    Idle -->|"allows processing only after idle threshold"| Processor
    Processor -->|"atomically store cleaned output + mark processed"| Cleaned
    Processor -->|"mark failed"| Transit
    Processor -->|"embed cleaned text"| EmbedProto
    EmbedProto -->|"384-float vector blob"| Memory
    Processor -->|"promote cleaned rows"| Memory
    Processor -->|"mark promoted"| Cleaned
    Slice -->|"search scoped memory"| Memory
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
    Overlay["Tauri Ambient Overlay"]
    Daemon["sovereignd"]
    Transit["SQLite Transit Buffer"]
    Cleaner["Local Cleaner / SLM Summarizer"]
    Embedder["Local Embedding Runtime\nONNX / ort"]
    MeGraph[".me Hybrid Vector Graph\nLanceDB"]
    Boundary["Need-to-Know Boundary Engine"]
    Meslice["Ephemeral .meslice"]
    Remote["External LLM / Agent Endpoint"]
    Watcher["Session Watcher"]
    Delta["Semantic Delta Extractor"]
    Reconcile["Graph Reconciliation + Decay"]

    OS --> Daemon
    Files --> Daemon
    Browser --> Daemon
    Overlay --> Boundary

    Daemon --> Transit
    Transit --> Cleaner
    Cleaner --> Embedder
    Embedder --> MeGraph

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
| `sovereignd` | Daemon crate | Implemented | `crates/sovereignd` | Local ingestion and transit-buffer orchestration. |
| `Workspace Manager` | Module | Implemented | `crates/sovereignd/src/workspace.rs` | Creates local Sovereign directories. |
| `SQLite Transit Buffer` | Local store | Implemented | `crates/sovereignd/src/transit.rs` | Stores captured raw text, queue status, retry counts, and stale processing lease repair. |
| `Cleaned Event Staging` | Local store | Implemented | `crates/sovereignd/src/transit.rs` | Stores normalized text ready for future embedding. |
| `Ingest Safety Filter` | Privacy guard | Implemented | `crates/sovereignd/src/ingest_safety.rs` | Blocks secret-bearing paths, private keys, credential markers, card-like numbers, bank-routing markers, and precise-location markers before SQLite persistence. |
| `Local Embedding Prototype` | Compute stage | Implemented prototype | `crates/sovereignd/src/embedding.rs` | Generates deterministic local 384-dimensional embeddings for promoted cleaned text. This is a Phase 1 spike, not the final ONNX embedding runtime. |
| `Identity Memory Store` | Local store | Implemented prototype | `crates/sovereignd/src/identity.rs` | Stores local `.me` memory nodes plus fixed-width vector blobs in `identity.me/state.db`; searches with vector similarity plus lexical scoring. |
| `.meslice Preview Generator` | Privacy boundary | Implemented prototype | `crates/sovereignd/src/slice.rs` | Builds scoped context blocks and prompt packages from local memory search. |
| `Local Capture Endpoint` | Ingestion adapter | Implemented | `crates/sovereignd/src/proxy.rs`, `crates/sovereignd/src/main.rs` | Accepts local HTTP captures at `127.0.0.1:8080`; non-loopback binds are rejected unless explicitly forced for local development. |
| `HTML/Text Cleaner` | Capture normalizer | Implemented | `crates/sovereignd/src/proxy.rs`, `lol-html` | Extracts visible document text through a lightweight streaming parser and ignores script/style raw text before transit persistence. |
| `Filesystem Poller` | Ingestion adapter | Implemented fallback | `crates/sovereignd/src/filesystem.rs` | Polls approved folders for conservative text/code files on a blocking worker, reusing one SQLite writer per scan; OS-native watchers are still planned. |
| `Idle Telemetry Gate` | Resource guard | Implemented minimal | `crates/sovereignd/src/idle.rs` | Gates processing by recent user input on Windows and falls open where OS telemetry is unavailable. |
| `Transit Processor` | Pipeline worker | Implemented placeholder | `crates/sovereignd/src/processor.rs` | Claims queued captures and marks processing result. |
| `Tauri Overlay` | UI shell | Planned | `docs/engineering-roadmap.md` | Ambient command interface. |
| `.me Vector Graph` | Durable local state | Planned | `docs/local-vector-synthesis-architecture.md` | Stores hybrid document-vector-graph state. |
| `Local Embedding Runtime` | Compute stage | Implemented prototype / final runtime planned | `crates/sovereignd/src/embedding.rs`, `docs/local-vector-synthesis-architecture.md` | Current implementation is deterministic local hashing; final ONNX/ort embedding runtime remains planned. |
| `Boundary Engine` | Privacy gate | Planned | `docs/ephemeral-handshake-architecture.md` | Chooses minimum context needed for a task. |
| `.meslice` | Ephemeral payload | Planned | `docs/ephemeral-handshake-architecture.md` | Task-bound context stream for external agents. |
| `Session Watcher` | Feedback observer | Planned | `docs/bidirectional-state-synchronization-architecture.md` | Captures scoped outputs from agent sessions. |
| `Semantic Delta Extractor` | Feedback processor | Planned | `docs/bidirectional-state-synchronization-architecture.md` | Converts session logs into structured state deltas. |
| `Graph Reconciliation` | State merger | Planned | `docs/bidirectional-state-synchronization-architecture.md` | Merges deltas and decays outdated edges. |

## 4. Local Workspace Ontology

```text
~/.sovereign/
  identity.me/   implemented prototype memory store directory
    state.db     implemented SQLite `.me` staging ledger with vector blobs
  transit.db     implemented SQLite transit buffer and cleaned staging
  logs/          reserved local daemon logs
```

## 5. Implemented Command Surface

| Command | Pipeline | Inputs | Writes | Current Purpose |
| :--- | :--- | :--- | :--- | :--- |
| `init` | Workspace bootstrap | None | `~/.sovereign/*` | Creates local workspace and transit DB. |
| `ingest` | Manual capture | `--source`, `--content` | `captured_events` | Queues a text event manually. |
| `list` | Inspection | None | None | Lists recent captured events. |
| `stats` | Inspection | None | None | Counts events by status. |
| `repair-transit` | Transit repair | `--lease-ms` | `captured_events.status`, `captured_events.retry_count` | Requeues stale `processing` claims after a bounded lease timeout. |
| `cleaned-list` | Inspection | `--limit` | None | Lists normalized text staged for vectorization. |
| `memory-list` | Inspection | `--limit` | None | Lists local identity memory nodes. |
| `memory-search` | Local retrieval | `--query`, `--limit` | None | Searches memory nodes by deterministic token overlap. |
| `slice-preview` | Context boundary | `--intent`, `--limit` | None | Emits an ephemeral context block without raw memory IDs, hashes, or scores. |
| `prompt-package` | Context injection artifact | `--intent`, `--prompt`, `--limit` | None | Emits a local prompt package containing scoped context plus user task. |
| `serve` | Local proxy capture | `--addr`, `--allow-non-loopback` | `captured_events` | Runs `/health` and `/capture`; defaults to loopback-only binding. |
| `watch` | Filesystem capture | `--path`, `--non-recursive` | `captured_events` | Polls approved folders for text files. |
| `process-once` | Transit processing | `--limit` | `captured_events.status`, `cleaned_events` | Claims captures and stages normalized text. |
| `process-idle-once` | Idle-gated transit processing | `--limit`, `--idle-ms` | `captured_events.status`, `cleaned_events` when idle | Runs one processing batch only after the configured idle threshold. |
| `promote-once` | Memory promotion | `--limit` | `identity.me/state.db`, `cleaned_events.promoted_at_ms` | Promotes cleaned rows into local memory nodes. |

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
    participant CLI as sovereignd ingest
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

    Client->>Proxy: POST /capture
    Proxy->>Cleaner: clean_payload(content_type, body)
    Cleaner-->>Proxy: cleaned text
    Proxy->>Transit: ingest_text(local-proxy:*, cleaned) with ingest safety filter
    Transit-->>Proxy: captured event id
```

### Filesystem Capture

```mermaid
sequenceDiagram
    participant Poller as Filesystem Poller
    participant FS as Approved Folder
    participant Transit as SQLite Transit Buffer

    Poller->>Poller: spawn blocking scan every 2s
    Poller->>Transit: open one TransitBuffer for scan pass
    Poller->>FS: walk approved root
    FS-->>Poller: text-like file metadata
    Poller->>Poller: check conservative extension, size, fingerprint
    Poller->>Transit: ingest_text(filesystem:path, cleaned) with ingest safety filter
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
    Worker->>Cleaner: clean_for_next_stage(content)
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

### Memory Promotion

```mermaid
sequenceDiagram
    participant Worker as promote-once
    participant Staging as Cleaned Event Staging
    participant Embed as Local Embedding Prototype
    participant Memory as Identity Memory Store

    Worker->>Staging: list_cleaned_pending(limit)
    Staging-->>Worker: oldest unpromoted cleaned events
    Worker->>Embed: embed cleaned text into 384-float vector
    Embed-->>Worker: little-endian vector blob
    Worker->>Memory: insert_memory_from_cleaned with vector blob
    Memory-->>Worker: memory node id
    Worker->>Staging: mark_cleaned_promoted
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
    Slice-->>CLI: SOVEREIGN-CONTEXT-BLOCK
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

## 8. Planned Pipeline Boundaries

| Boundary | Upstream | Downstream | Rule |
| :--- | :--- | :--- | :--- |
| Capture to transit | Proxy / filesystem / manual | Ingest safety filter, then SQLite transit buffer | Reject deterministic sensitive material before any SQLite persistence. |
| Transit to cleaned staging | SQLite transit buffer | Transit processor | Process only claimed events; successful cleaned writes and `processed` status updates share one SQLite transaction. |
| Cleaned staging to `.me` prototype | `cleaned_events` | Local embedding prototype, then identity memory store | Promote normalized text into local memory nodes with fixed-width vector blobs. |
| `.me` prototype to retrieval | Identity memory store | Local query caller | Return vector/lexical ranked memory nodes. |
| `.me` prototype to `.meslice` preview | Identity memory store | `.meslice` generator | Export bounded declarative summaries only; no raw DB ids, hashes, sources, or scores. |
| `.meslice` preview to prompt package | `.meslice` generator | Local caller | Combine scoped context with user prompt without network transmission. |
| `.me` prototype to vectorization | `identity.me/state.db` | Local embedder | Embed memory nodes, not raw capture rows. |
| Vectorization to `.me` graph | Embedder | LanceDB graph | Write structured memories, not raw telemetry. |
| `.me` to `.meslice` | Boundary engine | External agent | Export minimum declarative facts only. |
| External execution to feedback | Agent endpoint | Session watcher | Capture only scoped task outputs. |
| Feedback to `.me` | Delta extractor | Graph reconciler | Validate deltas before merge. |

## 9. Maintenance Rules

When changing the system, update this file in the same pass if any of the following change:

- A new crate, module, command, or local store is added.
- A pipeline gains or loses a stage.
- A state transition changes.
- A new external boundary appears.
- A planned component becomes implemented.
- A dependency changes the architecture or performance budget.

Keep this map factual. Mark future components as `Planned`; do not imply they are implemented before code exists.
