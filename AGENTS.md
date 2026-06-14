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
- Print CLI help through `--help`, `-h`, or `help` before opening or creating the default Identity workspace.
- Capture bounded local inputs through manual CLI, token-protected loopback HTTP, Windows foreground-window capture, and filesystem watching.
- Capture explicit user-selected browser/page text through an opt-in `capture-page` loopback helper or generated bookmarklet; do not make browser DOM capture ambient or automatic.
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
- Validate protocol-facing `.me` node shape through `doctor` and `memory-protocol-health` before expanding Phase 2 context streaming, including parseable JSON-object structured attributes rather than brace-shaped malformed strings.
- Repair bounded protocol-facing `.me` drift locally through `repair-protocol-schema`, including malformed UUIDs, timestamps, structured attributes, and vector dimensions.
- Protect captured text, source labels, cleaned staging text, and prototype `.me` semantic text fields before SQLite persistence, while preserving legacy plaintext reads for development data.
- Report and repair legacy plaintext development rows through `doctor` and `protect-at-rest`.
- Redact duplicate transit content after successful local `.me` promotion.
- Run the default local context daemon through `start`, which enables loopback capture, the idle-gated pipeline, bounded active-window metadata capture, and the global `Ctrl+Shift+I` clipboard hotkey.
- Include bounded recent selected-page memory in hotkey context only for browser/agent surfaces, detected by known browser process names or explicit agent-chat title markers, after normal profile/query memory search and under the same dedupe and budget rules; automatic selected-page fallback is capped to captures from the last 24 hours.
- Search active project profile memory terms together with the current window title, then rank hotkey context facts with a deterministic freshness/source-diversity pass so repeated foreground-window title memories collapse to one useful fact while project/profile facts, relevant memory hits, and eligible selected-page context can still share the bounded context budget; selected-page fallback and any one source domain must not monopolize fact slots on the first pass when other eligible facts are available, while still filling otherwise empty slots.
- Keep that fallback scoped to explicit selected-page captures with `Selected page text:` metadata. Generic token-protected loopback web captures may remain searchable local memory, but they must not be treated as selected-page context.
- Start Phase 3 feedback-loop work through explicit `agent-delta-schema`, `agent-delta-validate`, `agent-delta-preview`, `agent-delta-commit`, `agent-delta-list`, `agent-delta-show`, `agent-delta-stats`, and `agent-delta-edges` commands that expose the local reviewed-candidate contract, validate user-provided agent outcome text, normalize sources as bounded lowercase slugs under `agent-delta:`, emit a bounded structured schema-versioned candidate, allow reviewed candidate JSON to be revalidated and committed without re-extracting prose, reject unknown candidate JSON fields, flag sensitive review categories, optionally commit only that validated delta into local `.me` memory under `agent.outcome/AGENT_DELTA`, report/inspect protocol-facing node ids rather than internal SQLite row ids, report duplicate retries as `write_status=existing` without extra vector writes, support protocol-safe `agent-delta-commit --json`, inspect recent or single committed deltas with top-level outcome state, entities, extracted delta attributes, and review flags/categories but without raw text, hashes, vectors, scores, or internal SQLite row ids, inspect aggregate counts by outcome state/source/review category without summaries, entities, node ids, raw text, hashes, vectors, scores, or internal SQLite row ids, and inspect agent-delta graph edges through protocol-facing endpoint node ids rather than local row ids; `agent-delta-schema`, `agent-delta-validate`, and `agent-delta-preview` must run before workspace setup because they do not require `.me` access; `agent-delta-validate` must report compact validation status, including whether commit would require `--allow-sensitive`, without echoing summaries, entities, attributes, raw text, hashes, vectors, scores, or internal SQLite row ids; `agent-delta-commit` must parse/validate the candidate and fail sensitive categories without `--allow-sensitive` before workspace setup; `agent-delta-list`, `agent-delta-stats`, and `agent-delta-edges` must remain hard-capped at 100 rows, list/stats must support `--review-only` for review-required deltas, CLI-normalized `--review-category` for one sensitive review category, `--source` for one normalized `agent-delta:` source, `--entity` for one extracted entity, and CLI-normalized `--state` filters for one stored uppercase outcome state, malformed list/stats/edges `--limit`, `--source`, or `--entity` values and unknown list/stats/edges state or review-category filters must be rejected before workspace setup, `agent-delta-show` must require one UUIDv4-shaped protocol node id and return only the same protocol-safe committed-delta shape, edge inspection must additionally support those same delta filters plus one UUIDv4-shaped protocol node-id and CLI-normalized relationship filters while omitting raw text, summaries, hashes, vectors, scores, and internal SQLite row ids, missing or malformed show/edge protocol node-id filters and malformed edge relationship filters must be rejected before workspace setup, and accepted show/edge protocol node-id filters must normalize to canonical lowercase before lookup; commits that mention finance, health, legal identity, or private communications require `--allow-sensitive`, repeated identical commits must dedupe through the stable local delta id before embedding/vector writes, committed deltas should use concise source-specific summary tokens and structured attributes, committed deltas should reconcile to recent same-entity memories through bounded graph edges such as `OUTCOME_FOR`, `UPDATED_BY`, `SUPERSEDES`, and `SUPERSEDED_BY`, changed same-key delta attributes should add explicit `ATTRIBUTE_CONFLICTS_WITH` / `ATTRIBUTE_REPLACED_BY` edges, agent-delta node plus outcome, conflict, and supersession edge counts should be visible through `memory-graph-health`, newer same-entity deltas should apply bounded edge-weight decay to older delta outgoing non-supersession edges in the reconciliation transaction, and none of this should be treated as an ambient session watcher.
- Keep the default build lean: filesystem vector blobs plus SQLite fallback are the normal vector backend. LanceDB/Arrow are opt-in only through `--features lancedb-backend`.
- Keep Windows active-window deep UI Automation/MSAA text extraction opt-in only through `IDENTITYD_ENABLE_DEEP_ACTIVE_WINDOW_TEXT=1`; the default daemon captures stable foreground application/title metadata because the deep native path can access-violate when launched hidden.

Do not start by building UI, cloud sync, model orchestration, or cryptographic protocol machinery until the local daemon and ingestion pipeline are stable.

## Recent Stability And Debloat Decisions

These are current implementation facts, not wishlist items:

- `.\target\release\identityd.exe start` is the simplest visible daemon command. `.\start-identity.cmd` wraps it.
- `.\start-identity-hidden.cmd` starts the same default daemon as a hidden background process.
- `.\scripts\test-identity-hotkey.ps1` is the local end-to-end self-test. It starts a temporary hidden daemon, checks `/health`, simulates `Ctrl+Shift+I`, verifies an `IDENTITY-CONTEXT-BLOCK` lands on the clipboard, restores the clipboard, and stops the temporary daemon.
- `.\scripts\test-identity-page-capture.ps1` is the focused explicit browser/page capture self-test. It starts a temporary loopback capture server on an ephemeral local port, verifies plain clipboard text is rejected by `capture-page --from-clipboard`, copies an `IDENTITY-PAGE-CAPTURE` clipboard envelope, runs `capture-page --from-clipboard --promote-now`, verifies the selected page capture is searchable, temporarily gives the test terminal a browser/agent-like title, verifies the selected page is included in a `context-now --preview --project tfl-central` block from the temporary `.me` store, restores the clipboard/window title, and stops the temporary server.
- The default hotkey is `Ctrl+Shift+I`, copy-only. `--paste-on-hotkey` remains opt-in and must never press Enter.
- Browser/page capture is opt-in and user-triggered. `capture-page` sends selected text, title, and URL to the existing token-protected loopback `/capture` endpoint, storing page URLs only for `http://` and `https://` pages and without query strings or fragments; `capture-page --from-clipboard` requires an `IDENTITY-PAGE-CAPTURE` envelope from the local clipboard rather than accepting arbitrary clipboard text; `capture-page --promote-now` may process and promote only the just-queued explicit capture for immediate hotkey context; `browser-capture-clipboard-bookmarklet` is the preferred no-token browser bridge and only copies `window.getSelection()` plus title and URL to the clipboard. `browser-capture-bookmarklet` remains available for direct loopback posting but prompts for the capture token in page context.
- Long-running daemon logging paths must be non-panicking. Do not use raw `println!`/`eprintln!` in daemon hot paths where broken hidden-process stdout/stderr handles could kill the process.
- The default release build intentionally excludes LanceDB/Arrow. Measured lean default release size was about 2.2 MB versus about 29.8 MB with the previous LanceDB-default build. Do not re-enable `lancedb-backend` by default without explicit human approval.
- The Codex in-app Browser plugin may fail on this Windows desktop with an `AttachConsole failed` / `windows sandbox failed: spawn setup refresh` error before it opens a tab. Treat that as a Codex desktop Browser bridge issue, not as proof that `identityd` is broken. Prefer the local `/health` endpoint and `scripts/test-identity-hotkey.ps1` for daemon verification.

## Implementation Stack Constraints

Use these defaults unless a human explicitly changes the architecture:

- Core daemon: Rust, edition 2021.
- Async runtime: `tokio` for proxy traffic, filesystem listeners, and concurrent background work.
- Desktop shell: Tauri v2, Rust backend with HTML/TypeScript frontend.
- Ambient daemon memory target: under 50MB.
- Hidden overlay memory target: under 40MB.
- Transit buffer: SQLite for queued raw captures.
- Embedded vector store: filesystem vector blobs plus SQLite fallback by default; LanceDB/Arrow only behind the explicit `lancedb-backend` feature.
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
    -> filesystem vector blobs + SQLite prototype graph
       (optional LanceDB backend only with --features lancedb-backend)
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
| 6 | Store | filesystem vector blobs + SQLite graph | Under 10ms |

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

Phase 2 V0 hotkey context injection is implemented. Future work should preserve this working baseline before expanding scope.

1. **Do not rebuild UI first.** Keep the current clipboard-first daemon path stable before adding any Tauri overlay, dashboard, cloud sync, or model orchestration.
2. **Preserve the lean default.** Normal `cargo build --release -p identityd` must use filesystem vectors plus SQLite fallback. `lancedb-backend` and `onnx-runtime` stay explicit opt-in feature flags.
3. **Validate daemon behavior locally.** Run `.\scripts\test-identity-hotkey.ps1` after any hotkey, clipboard, active-window, daemon, or startup change. It is the canonical local V0 smoke test.
4. **Validate explicit page capture locally.** Run `.\scripts\test-identity-page-capture.ps1` after any `capture-page`, browser bookmarklet, clipboard-envelope, loopback `/capture`, exact-promotion, or recent selected-page context change.
5. **Measure bloat.** Run `cargo tree -p identityd --edges normal`, `cargo test -p identityd`, and `cargo build --release -p identityd`; check `(Get-Item target/release/identityd.exe).Length`. The default binary target remains under 15 MB.
6. **Keep active-window capture safe.** Do not enable deep Windows UI Automation/MSAA text extraction by default. The safe default is foreground app and title metadata; deeper text extraction is only for explicit local debugging with `IDENTITYD_ENABLE_DEEP_ACTIVE_WINDOW_TEXT=1`.
7. **Keep browser/page capture explicit.** Prefer the clipboard-envelope bridge: browser copies selected page text/title/URL, then local `capture-page --from-clipboard` authorizes through the workspace token. Do not add headless browsers, browser automation runtimes, full DOM scraping, or ambient browser surveillance.
8. **Update `docs/system-map.md`, `README.md`, and this file** whenever command defaults, runtime boundaries, dependency defaults, daemon state transitions, or capture surfaces change.

Current V0 definition of done: `context-now --preview` prints, `context-now --copy` writes clipboard, `start` runs the default local context daemon, and pressing `Ctrl+Shift+I` inside Gemini/Codex/Antigravity copies a compact context block with active-window metadata and relevant memory excerpts when available. No dashboard. No Tauri. No network. Binary and RAM budgets remain hard constraints.

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
