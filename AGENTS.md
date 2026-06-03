# Identity Coding Agent Specification

This file is the canonical implementation brief for AI coding agents working in this repository.

Identity is a local-first identity and context protocol. The user owns a dynamic encrypted `.me` state bank. External agents must never receive the full `.me` graph; they receive only scoped, ephemeral `.meslice` payloads.

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

The first implementation lives in `crates/identityd`.

Current responsibility:

- Create the local Identity workspace.
- Create `~/.identity/identity.me`.
- Create `~/.identity/transit.db`.
- Store captured raw text events in a local SQLite transit buffer.
- Capture bounded local inputs through manual CLI, token-protected loopback HTTP, Windows foreground-window capture, and filesystem watching.
- Report capture-adapter readiness through the centralized `capture.rs` health boundary rather than duplicating status logic in CLI code.
- Refuse broad or sensitive filesystem watch roots by default; require an explicit unsafe development flag before watching the home directory, ledger workspace, credentials directories, AppData, Windows, or Program Files.
- Process queued captures through an idle-gated local pipeline.
- Promote cleaned captures into the prototype local `.me` memory store with fixed-width local embeddings, vector blob mirroring, and prototype weighted graph edges.
- Verify and restore the primary local vector mirror instead of relying on SQLite fallback reads to mask missing vector files.
- Keep the embedding runtime boundary explicit through `embedding.rs` metadata, local ONNX artifact preflight, local WordPiece tokenization, and feature-gated ONNX Runtime session health/execution; `doctor` scores the configured artifact separately from the final runtime, `embedding-manifest-write` can create the adjacent `<model>.onnx.identity.json` manifest declaring the persisted embedding dimension, `embedding-tokenizer-health` validates a local `vocab.txt`, `embedding-tokenize` emits local `input_ids`, `attention_mask`, and `token_type_ids`, `onnx-runtime-health` can validate session loading when compiled with `--features onnx-runtime`, `embedding-onnx-run` can execute an explicit local ONNX embedding smoke path, and `IDENTITY_EMBEDDING_RUNTIME=onnx` lets the live `EmbeddingEngine` attempt ONNX during promotion/search with hash fallback if the local runtime is unavailable.
- Keep embedding model-family metadata honest: empty `.me` stores may adopt a healthy active ONNX manifest model id, but non-empty hash-backed stores must remain hash-backed until an explicit local re-embedding migration exists.
- Assign every prototype `.me` memory node a UUIDv4-style `node_uid` for protocol-facing identity, while retaining compact SQLite row ids for local joins.
- Persist UTC ISO8601 creation and last-access protocol timestamps for prototype `.me` memory nodes, while retaining millisecond epochs for efficient local ordering.
- Export recent prototype `.me` nodes through a local protocol-shaped JSON command for inspection, using protocol-facing node ids rather than internal SQLite row ids.
- Validate protocol-facing `.me` node shape through `doctor` and `memory-protocol-health` before expanding Phase 2 context streaming.
- Repair bounded protocol-facing `.me` drift locally through `repair-protocol-schema`, including malformed UUIDs, timestamps, structured attributes, and vector dimensions.
- Protect captured text, source labels, cleaned staging text, and prototype `.me` semantic text fields before SQLite persistence, while preserving legacy plaintext reads for development data.
- Report and repair legacy plaintext development rows through `doctor` and `protect-at-rest`.
- Redact duplicate transit content after successful local `.me` promotion.

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
[IDENTITY-CONTEXT-BLOCK: ID_884920]
- Ephemeral tokenized context segment payload
- Authorization expiry signature: POSIX_TIMESTAMP + 2000MS
[IDENTITY-CONTEXT-BLOCK-END: ID_884920]
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

- Build `identityd`.
- Establish local workspace and transit buffer.
- Add filesystem/accessibility/proxy ingestion.
- Add local embedding and LanceDB storage.

Phase 2, days 31-60:

- Build lightweight hotkey context injection daemon (no Tauri overlay required for V0).
- Add `context_snapshot.rs` to read active window metadata on demand without transit queuing.
- Add `project_profile.rs` to load deterministic project profiles from `~/.identity/projects.json`.
- Add `context_builder.rs` to combine snapshot, profile, and memory into a bounded `[IDENTITY CONTEXT]` block.
- Add `clipboard.rs` to write to the Windows clipboard natively via Win32 API.
- Add `hotkey.rs` to register a global system hotkey via `RegisterHotKey` and trigger context injection.
- Add `context-now` CLI command with `--preview` and `--copy` modes.
- Add `project-profile-list` CLI command.
- Extend `daemon` with `--hotkey`, `--hotkey-combo`, and `--paste-on-hotkey` flags.
- Seed a `tfl-central` example project profile in `~/.identity/projects.json`.

Phase 3, days 61-90+:

- Add Session Watcher Daemon.
- Extract reverse semantic deltas.
- Reconcile graph state.
- Implement decay.
- Explore TEE integrations.

## Next Agent Execution Steps

Phase 1 is complete. Begin Phase 2 Hotkey Context Injection. The next code changes should be, in order:

1. **Audit before building.** Inspect the repo and confirm: which function powers `capture-active-window`, which function powers `memory-search`, what `slice.rs` exposes, whether a clipboard utility exists, and whether `RegisterHotKey` can be called without the `windows` crate.
2. **Add `context-now --preview`.** Implement `context_snapshot.rs`, `project_profile.rs`, and `context_builder.rs` as a minimal chain. Wire into a new `context-now` CLI command. Print the context block to stdout. No clipboard, no hotkey.
3. **Add project profile loading.** Support `~/.identity/projects.json` with deterministic substring matching. Add `project-profile-list` command. Add a `tfl-central` seed profile with its guardrails and memory query terms.
4. **Add `context-now --copy`.** Implement `clipboard.rs` with native Win32 `OpenClipboard`/`SetClipboardData`/`CloseClipboard`. Log a short confirmation line.
5. **Add `daemon --hotkey`.** Implement `hotkey.rs` with native Win32 `RegisterHotKey`/`GetMessage` on a dedicated thread. Debounce rapid repeats. On press, call the same internal path as `context-now --copy`. Do not block the ingestion pipeline.
6. **Add `--paste-on-hotkey` (opt-in).** Use native `SendInput` or `keybd_event` to simulate Ctrl+V after clipboard write. Default remains copy-only. Paste must never press Enter.
7. **Update tests.** Cover project profile matching, context budget trimming, no raw IDs in output, guardrails present when project matches, empty memory results, missing active-window data, and prompt-injection sanitization.
8. **Update `docs/system-map.md` and `README.md`** to mark Phase 2 modules as implemented once built.

V0 definition of done: `context-now --preview` prints, `context-now --copy` writes clipboard, `daemon --hotkey` responds to hotkey. Pressing the hotkey inside Gemini/Codex/Antigravity copies a compact context block with active-window context, TfL guardrails (when matched), and relevant memory excerpts. No dashboard. No Tauri. No network. Binary and RAM budgets must remain within current limits.

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
