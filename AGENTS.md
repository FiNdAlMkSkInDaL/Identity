# Sovereign Coding Agent Specification

This file is the canonical implementation brief for AI coding agents working in this repository.

Sovereign is a local-first identity and context protocol. The user owns a dynamic encrypted `.me` state bank. External agents must never receive the full `.me` graph; they receive only scoped, ephemeral `.meslice` payloads.

## Macro Architecture

```text
[User Input]
    |
    v
[Universal UI / Tauri Overlay Window]
    |
    | Local cryptographic request
    v
[Local .me State Bank / LanceDB Hybrid Graph]
    |
    | Token-optimized context stream
    v
[Stateless LLM / Autonomous Agent Engine]
    |
    | Executes actions on API / web layers
    v
[Target Sites]
    |
    v
[Wiped Post-Execution]
```

## Current Code Spine

The first implementation lives in `crates/sovereignd`.

Current responsibility:

- Create the local Sovereign workspace.
- Create `~/.sovereign/identity.me`.
- Create `~/.sovereign/transit.db`.
- Store captured raw text events in a local SQLite transit buffer.

Do not start by building UI, cloud sync, model orchestration, or cryptographic protocol machinery until the local daemon and ingestion pipeline are stable.

## Implementation Stack Constraints

Use these defaults unless a human explicitly changes the architecture:

- Core daemon: Rust, edition 2021.
- Async runtime: `tokio` for proxy traffic, filesystem listeners, and concurrent background work.
- Desktop shell: Tauri v2, Rust backend with HTML/TypeScript frontend.
- Ambient daemon memory target: under 50MB.
- Hidden overlay memory target: under 40MB.
- Transit buffer: SQLite for queued raw captures.
- Embedded vector store: LanceDB, in process, zero-server.
- Local inference: ONNX Runtime or `ort` Rust bindings.
- Acceleration targets: CoreML on Apple Silicon, DirectML on Windows, WebGPU where appropriate.
- Quantization: prefer FP16 or INT4 profiles for local models.
- Local proxy: bind to `127.0.0.1:8080`.
- Network timeouts: cap upstream routing operations at 3000ms.

Avoid adding alternative databases, cloud queues, hosted vector stores, or server-first infrastructure unless the roadmap is intentionally revised.

See `docs/agent-constraints-and-performance-budget.md` for the hard dependency policy, resource budgets, and anti-bloat guardrails. When this file and that supplement appear to conflict, choose the leaner implementation and ask a human before adding dependencies outside the approved core.

Maintain `docs/system-map.md` as the living ontology of the system. Update it in the same implementation pass whenever a module, command, data store, pipeline stage, state transition, or external boundary changes.

## Master `.me` Storage Schema

The local `.me` store is a hybrid document-vector-graph. Entries should conform to this shape:

```json
{
  "node_id": "UUIDv4",
  "timestamp_created": "ISO8601_Z",
  "timestamp_last_accessed": "ISO8601_Z",
  "domain_context": "professional.networking",
  "entity_type": "PERSON",
  "semantic_payload": {
    "raw_text": "Clean Markdown source data",
    "summary_tokens": "SLM-generated concise declaration",
    "structured_attributes": {
      "key": "value"
    }
  },
  "vector_embedding": [0.0],
  "graph_edges": [
    {
      "target_node_id": "UUIDv4",
      "relationship_type": "EMPLOYED_BY",
      "edge_weight": 0.87
    }
  ]
}
```

Rules:

- `node_id` must be UUIDv4.
- Timestamps must be UTC ISO8601.
- `vector_embedding` must match the active model dimension, such as 384 for MiniLM-L6-v2 or 1024 for larger BGE models.
- `edge_weight` must stay in the inclusive range `0.0..=1.0`.
- Old context should not be destructively overwritten. Use time-aware edges and decay.

## `.meslice` Transit Schema

External LLMs and agents receive `.meslice` payloads, never the raw `.me` graph.

Never include:

- Raw `node_id` values.
- Raw tracking telemetry.
- Unmasked persistent identifiers.
- Secrets, passwords, private keys, or raw financial tokens.

Transit shape:

```json
{
  "session_token": "CRYPTOGRAPHIC_SALT_STRING",
  "expiry_epoch": 1780000000,
  "injected_context": [
    {
      "context_group": "professional.outreach",
      "declarative_facts": [
        "The user is currently communicating with an early-stage venture investor.",
        "The user communicates using a direct, concise tone; no corporate jargon."
      ]
    }
  ]
}
```

## Core Technical Challenges

### 1. Local Vector Synthesis

Raw user data must not be sent to the cloud to build `.me`.

Expected pipeline:

```text
OS activity / filesystem / local proxy
    -> Rust daemon capture
    -> SQLite transit buffer
    -> idle-aware local SLM cleaner
    -> local embedding model
    -> LanceDB hybrid vector graph
```

Capture sources:

- Windows UI Automation.
- macOS `NSAccessibility`.
- Linux accessibility APIs where viable.
- `ReadDirectoryChangesW`, `FSEvents`, and `inotify` for filesystem watching.
- Optional loopback proxy on `127.0.0.1:8080`.

Throttle local inference. Process buffered data only during idle or low-pressure windows.

### 2. Cryptographic Context Fragmentation

External agents are untrusted.

Expected flow:

```text
External prompt
    -> local intent parser
    -> need-to-know boundary filter
    -> targeted `.me` query
    -> in-memory `.meslice`
    -> client-side injection or secure enclave route
    -> expiry and purge
```

Client-side injection envelope:

```text
[SOVEREIGN-CONTEXT-BLOCK: ID_884920]
- Ephemeral tokenized context segment payload
- Authorization expiry signature: POSIX_TIMESTAMP + 2000MS
[SOVEREIGN-CONTEXT-BLOCK-END: ID_884920]
```

TEE pathway is later-stage protocol work. Do not make it an MVP dependency.

### 3. Bi-Directional State Synchronization

Agent task outcomes must write back into local state.

Expected feedback loop:

```text
Session watcher
    -> payload shadowing
    -> volatile execution log
    -> semantic delta extraction
    -> validation
    -> graph reconciliation
    -> edge-weight decay
```

Raw logs are volatile. Commit only validated semantic deltas.

## Security Blacklist

If an outbound agent request asks for any of these categories, block the request and surface a security warning:

- Master cryptographic private keys.
- System passwords.
- `.env` files.
- Raw unencrypted banking tokens.
- Routing numbers.
- Credit card values.
- Persistent biometric markers.
- Explicit physical location data, unless the user grants single-session consent.

Security behavior must be deterministic. Do not rely only on model judgment for these categories.

## Edge-Weight Decay

When the reverse delta engine writes an updated attribute that conflicts with an existing node, apply decay inside the database transaction:

```text
Weight_next = Weight_current * (1 - alpha)
```

Coefficient rules:

- If `delta_t < 24 hours`, `alpha = 0.1`.
- If `delta_t >= 24 hours`, `alpha = 0.4`.

Clamp resulting weights to `0.0..=1.0`.

## Defensive Fallbacks

### Local Inference Pressure

If ONNX/WebGPU/CoreML/DirectML inference runs out of memory or system latency exceeds 200ms:

- Stop the hardware-accelerated inference path safely.
- Downgrade to CPU-bound inference with a lower-dimensional fallback model.
- Throttle SQLite ingestion processing by 300%.
- Keep raw strings queued on disk until telemetry normalizes.

### Proxy Disconnect

If the local proxy drops or a website prevents loopback routing:

- Pass native traffic through to the normal network path.
- Do not break the user's internet access.
- Notify the user through the Tauri desktop shell.
- Enter detached, secure, offline read-only mode for capture.

## Runtime Latency Targets

### Ingestion

| Stage | Action | Pipeline | Target |
| :--- | :--- | :--- | :--- |
| 1 | Intercept | Rust daemon / local proxy | Under 5ms |
| 2 | Buffer | SQLite cache DB | Under 1ms |
| 3 | Trigger | OS telemetry watchdog | Async |
| 4 | Purify | Local 1B-3B SLM | Under 1.5s burst |
| 5 | Map | Local embedding model | Under 200ms burst |
| 6 | Store | LanceDB `.me` graph | Under 10ms |

### Handshake

| State | Operation | Location | Security |
| :--- | :--- | :--- | :--- |
| 1 | Intercept agent request | Local transport layer | Private / encrypted |
| 2 | Parse minimum nodes | Local parsing SLM | Isolated |
| 3 | Generate `.meslice` | RAM | Single-use |
| 4 | Execute prompt | Browser stream or TEE | Transit encrypted |
| 5 | Terminate | Memory watchdog | Purged |

### Feedback

| Phase | Operation | Mechanism | State |
| :--- | :--- | :--- | :--- |
| 1 | Shadow outputs | Session watcher hooks | Volatile |
| 2 | Extract variables | Local SLM | Volatile |
| 3 | Reconcile graph | LanceDB / DuckDB write | Persisted |
| 4 | Decay old edges | Chronological graph update | Persisted |

## Roadmap

Phase 1, days 1-30:

- Build `sovereignd`.
- Establish local workspace and transit buffer.
- Add filesystem/accessibility/proxy ingestion.
- Add local embedding and LanceDB storage.

Phase 2, days 31-60:

- Build Tauri hotkey overlay.
- Add intent parsing and boundary engine.
- Generate in-memory `.meslice` payloads.
- Inject scoped context into model calls.

Phase 3, days 61-90+:

- Add Session Watcher Daemon.
- Extract reverse semantic deltas.
- Reconcile graph state.
- Implement decay.
- Explore TEE integrations.

## Next Agent Execution Steps

Start from the existing `sovereignd` crate. The next code changes should be:

1. Add `tokio` to `crates/sovereignd`.
2. Introduce an async daemon entrypoint.
3. Add a local loopback proxy skeleton on `127.0.0.1:8080`.
4. Parse `text/html` responses into clean text using a conservative HTML parser.
5. Write cleaned payloads into the existing SQLite transit buffer.

Do not skip ahead to `.meslice`, UI, or vector DB until the local capture and buffer loop is working.

## Reference Documents

- `README.md`
- `docs/manifesto.md`
- `docs/system-map.md`
- `docs/technical-challenges-and-moats.md`
- `docs/strategic-threat-vector-analysis.md`
- `docs/local-vector-synthesis-architecture.md`
- `docs/ephemeral-handshake-architecture.md`
- `docs/bidirectional-state-synchronization-architecture.md`
- `docs/engineering-roadmap.md`
