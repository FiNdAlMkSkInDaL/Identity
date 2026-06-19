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
cargo run -p identityd -- --help
cargo run -p identityd -- init
cargo run -p identityd -- --root C:\Temp\identity-dev doctor
cargo run -p identityd -- ingest --source manual --content "User prefers local-first systems."
cargo run -p identityd -- capture-active-window
cargo run -p identityd -- capture-page --title "Identity notes" --url "https://example.test/notes" --text "Selected page text to remember."
cargo run -p identityd -- capture-page --title "Identity notes" --url "https://example.test/notes" --stdin
cargo run -p identityd -- capture-page --from-clipboard
cargo run -p identityd -- capture-page --from-clipboard --promote-now
cargo run -p identityd -- browser-capture-bookmarklet
cargo run -p identityd -- browser-capture-clipboard-bookmarklet
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
cargo run -p identityd -- agent-delta-list --limit 10
cargo run -p identityd -- agent-delta-list --review-only --limit 10
cargo run -p identityd -- agent-delta-list --review-category finance --limit 10
cargo run -p identityd -- agent-delta-list --source follow-up --limit 10
cargo run -p identityd -- agent-delta-list --entity "Acme Capital" --limit 10
cargo run -p identityd -- agent-delta-list --state paid --limit 10
cargo run -p identityd -- agent-delta-show --node-id <uuid>
cargo run -p identityd -- agent-delta-stats --limit 100
cargo run -p identityd -- agent-delta-summary --review-only --limit 100
cargo run -p identityd -- agent-delta-stats --review-only --review-category finance --source billing --state PAID --limit 100
cargo run -p identityd -- agent-delta-edges --limit 50
cargo run -p identityd -- agent-delta-edges --relationship OUTCOME_FOR --limit 50
cargo run -p identityd -- agent-delta-edges --review-category finance --source billing --state paid --relationship OUTCOME_FOR --limit 50
cargo run -p identityd -- agent-delta-schema
cargo run -p identityd -- agent-delta-validate --candidate-json-stdin
cargo run -p identityd -- agent-delta-preview --source follow-up --text "Sent follow-up to Acme Capital. Confirmation reference: MSG-42"
cargo run -p identityd -- agent-delta-preview --candidate-json-stdin
cargo run -p identityd -- agent-delta-commit --source follow-up --text "Sent follow-up to Acme Capital. Confirmation reference: MSG-42"
cargo run -p identityd -- agent-delta-commit --candidate-json-stdin
cargo run -p identityd -- agent-delta-commit --json --source follow-up --text "Sent follow-up to Acme Capital. Confirmation reference: MSG-42"
cargo run -p identityd -- agent-delta-commit --source follow-up --allow-sensitive --text "Paid invoice for Acme Capital. Receipt reference: INV-42"
cargo run -p identityd -- serve
cargo run -p identityd -- watch --path C:\Users\finph\Documents
cargo run -p identityd -- watch --path C:\Users\finph\Documents --poll

# Phase 2 hotkey context injection
.\start-identity.cmd
.\start-identity-hidden.cmd
.\scripts\test-identity-hotkey.ps1
.\scripts\test-identity-page-capture.ps1
.\target\release\identityd.exe start
cargo run -p identityd -- context-now --preview
cargo run -p identityd -- context-now --copy
cargo run -p identityd -- context-now --preview --project tfl-central
cargo run -p identityd -- project-profile-list
cargo run -p identityd -- daemon --watch-active-window --hotkey --hotkey-combo "Ctrl+Shift+I"
cargo run -p identityd -- daemon --watch-active-window --hotkey --hotkey-combo "Ctrl+Shift+I" --paste-on-hotkey
```

`--help`, `-h`, and `help` print command help before opening or creating a
workspace, so help inspection is read-only.

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
cargo build --release -p identityd --features lancedb-backend
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

For explicit browser/page context, `capture-page` is a CLI-friendly loopback
helper. It accepts only user-provided selected text plus optional page title and
URL, validates the assembled payload locally, reads the workspace capture token,
and posts to the same token-protected `/capture` endpoint. Page URLs are stored
only for `http://` and `https://` pages, without query strings or fragments, to
avoid persisting local file paths, session tokens, or tracking parameters:

```powershell
$selection = Get-Clipboard
cargo run -p identityd -- capture-page --title "Identity notes" --url "https://example.test/notes" --text $selection
Get-Clipboard | cargo run -p identityd -- capture-page --title "Identity notes" --url "https://example.test/notes" --stdin
```

The safer browser bridge is `browser-capture-clipboard-bookmarklet`: it prints a
tiny bookmarklet that copies only the selected page text, title, and URL into an
`IDENTITY-PAGE-CAPTURE` clipboard envelope. It never sends a network request and
never asks for the capture token inside page JavaScript. After using it, run:

```powershell
cargo run -p identityd -- capture-page --from-clipboard
```

`--from-clipboard` accepts only that `IDENTITY-PAGE-CAPTURE` envelope. To capture
plain clipboard text manually, pipe it through `--stdin` or pass it with `--text`
instead; this keeps accidental clipboard contents out of the page-capture path.

Add `--promote-now` when the selected page should be available to the next
hotkey context immediately. This processes and promotes only the capture that
was just queued, rather than draining unrelated transit backlog; the default
daemon path remains idle-gated.

`browser-capture-bookmarklet` prints a tiny bookmarklet for selected browser
text. The bookmarklet prompts for the local capture token on each use, sends only
`window.getSelection()` plus page title and URL, and relies on `/capture` for
token auth, CORS preflight, textual media-type enforcement, the 1MB body budget,
and the shared deterministic safety blacklist. It is user-triggered only; it does
not perform ambient DOM watching or full-page scraping.

When `Ctrl+Shift+I` is pressed inside a known browser process or an agent chat
surface with an explicit agent title marker, the context builder can add a
bounded handful of recent selected-page captures after normal profile/query
memory search. This improves context quality for generic window titles without
making browser capture automatic. Automatic selected-page fallback is limited to
captures from the last 24 hours; older page captures remain searchable local
memory but are not injected just because the user is in a browser or agent
surface. Generic loopback web captures remain searchable local memory, but they
are not used by this recent selected-page fallback unless they carry the
explicit `Selected page text:` capture shape.

Before formatting the compact context block, the builder searches active project
profile memory terms together with the current window title, then applies a cheap
deterministic freshness/source-diversity ranking pass. Repeated foreground-window
title memories are collapsed to one useful fact, while distinct project/profile
memory hits and eligible recent selected-page captures can still share the
bounded context budget. Eligible selected-page fallback is capped so it cannot
monopolize fact slots when non-page facts are available, and one source domain
cannot consume every slot on the first pass when another eligible domain is
present; that domain cap relaxes only to fill slots that would otherwise stay
empty. This ranking uses only local normalized text, source domain, and
timestamps; it does not call a model or expand capture scope. If a high-ranked
fact does not fit the remaining character budget, the builder keeps scanning for
shorter later facts instead of ending the context list early.

All capture paths share the same transit safety gate before SQLite persistence:
capture content is capped at 1MB, source labels are capped at 2048 bytes, and
deterministic secret, credential, payment-card, routing, and precise-location
markers are rejected.

Phase 3 feedback-loop work starts with an explicit local delta boundary rather
than an ambient watcher. `agent-delta-preview` accepts outcome text through
`--text`, `--content`, or `--stdin`, applies the same deterministic safety gate
plus the outbound security blacklist, and emits a bounded structured JSON
candidate with `schema_version`, outcome state, summary, obvious entities,
key/value attributes, `requires_review`, and any review-required categories. It
can also validate a reviewed candidate through `--candidate-json <json>` or
`--candidate-json-stdin`, rejecting unknown fields, unsafe content, and
under-declared sensitive review categories.
`agent-delta-schema` prints the local reviewed-candidate contract as JSON,
including the schema version, source prefix, allowed outcome states, allowed
review categories, field limits, validation rules, and a candidate template.
`agent-delta-validate` accepts the same text or candidate-JSON inputs as preview
and reports a compact validation JSON object with source, outcome state,
review-required flags/categories, whether commit would require
`--allow-sensitive`, and entity/attribute counts, without writing and without
echoing summaries, entities, attributes, or raw text.
`agent-delta-schema`, `agent-delta-validate`, and `agent-delta-preview` run
before workspace setup because they only inspect the candidate contract or
validate user-provided text/JSON. They do not create or touch a ledger.
Missing candidate input values, including a following flag where a value should
be, are rejected on that pre-workspace path. Candidate input modes are mutually
exclusive, so text, stdin, and candidate JSON inputs cannot be combined.
`--source` is only accepted with text, content, or stdin extraction; reviewed
candidate JSON must carry its own `source` field. Duplicate single-value
candidate flags such as `--text` and `--candidate-json` are rejected rather than
silently choosing one value.
`agent-delta-commit` performs the same extraction or candidate-JSON validation,
validates the candidate against the local delta schema before workspace setup,
and writes only that validated candidate into local `.me` memory through the
existing embedding, vector mirror, and at-rest protection path.
Sources are normalized as bounded lowercase slugs under the `agent-delta:`
prefix and committed memories are classified as `agent.outcome/AGENT_DELTA`.
Finance, health, legal identity, and private-communication outcome markers fail
closed before ledger setup unless `--allow-sensitive` is passed after review.
Repeating the same
committed delta reuses a stable local
cleaned-event id so duplicate detection returns the existing memory node before
embedding/vector writes and retries do not bloat memory or spend unnecessary
local inference work. After commit, a bounded deterministic reconciliation pass
links the agent outcome to recent local memories that mention the same extracted
entity with `OUTCOME_FOR` / `UPDATED_BY` graph edges, and links newer matching
outcome deltas to older ones with `SUPERSEDES` / `SUPERSEDED_BY`. If two
same-entity outcome deltas share an attribute key with a changed value, the graph
also records `ATTRIBUTE_CONFLICTS_WITH` / `ATTRIBUTE_REPLACED_BY` edges so the
change is explicit without deleting either state; `memory-graph-health` reports
agent-delta node, outcome-edge, conflict-edge, and supersession-edge counts for
quick local inspection. When a newer outcome supersedes an older same-entity outcome, the
reconciliation transaction also applies the documented short/long edge-weight
decay formula to the older delta's outgoing non-supersession edges, so stale
outcome evidence fades without deleting local history. Committed delta nodes use concise
source-specific summary tokens and structured attributes for outcome state,
source, summary, entities, review categories, and extracted delta attributes, so
local search/export remains useful without exposing raw session logs.
`agent-delta-commit` reports the protocol-facing node id for the committed
memory, not the local SQLite row id, and includes `write_status=created` or
`write_status=existing` so duplicate retries are visible without extra writes;
passing `--json` returns the same bounded protocol-facing commit receipt as
JSON without echoing the summary, entities, attributes, raw text, hashes,
vectors, or internal row ids.
For a reviewed candidate JSON round trip, keep the object explicit and bounded:

```powershell
$candidate = @'
{
  "schema_version": 1,
  "source": "agent-delta:follow-up",
  "outcome_state": "SENT",
  "summary": "Sent the reviewed follow-up to Acme Capital.",
  "entities": ["Acme Capital"],
  "attributes": [
    { "key": "confirmation_reference", "value": "MSG-42" }
  ],
  "requires_review": false,
  "review_required_categories": []
}
'@
$candidate | cargo run -p identityd -- agent-delta-validate --candidate-json-stdin
$candidate | cargo run -p identityd -- agent-delta-commit --candidate-json-stdin --json
```

Unknown candidate fields are rejected, reviewed summary/entity/attribute-value
strings must already be trimmed, single-line, and bounded, and reviewed JSON
must include any sensitive review categories inferred from its outcome state,
summary, entities, and attributes. Sensitive review categories still require
`--allow-sensitive` before commit.
`agent-delta-list` emits recent committed deltas as bounded JSON with
protocol-facing node ids, UTC timestamps, source, outcome state, extracted
entities, extracted delta attributes, summary, and structured attributes, plus
top-level `requires_review` and review-category fields; it does not include raw
text, hashes, vector blobs, scores, or internal SQLite row ids. For committed
agent deltas, source labels are canonicalized and summary/structured-attribute
fields are reconstructed from bounded agent-delta labels rather than trusted
from arbitrary stored metadata.
Use `--review-only` to show only committed deltas that require explicit review,
`--review-category <category>` to inspect one sensitive review category such as
`finance`, `health`, `legal_identity`, or `private_communications`,
`--source <label>` to inspect one normalized `agent-delta:` source, and
`--entity <name>` to inspect deltas for one extracted entity, and
`--state <STATE>` to inspect one outcome state such as `SENT`, `PAID`, or
`FAILED`; state filters are normalized at the CLI boundary, so lowercase,
kebab-case, and whitespace-separated input still maps to the strict stored
uppercase state labels. Unknown state and review-category filters are rejected
before workspace setup, and `--source` / `--entity` must include explicit
non-empty values when present. Duplicate single-value filters such as `--limit`,
`--source`, `--entity`, `--state`, `--review-category`, `--node-id`, and
`--relationship` are rejected before workspace setup instead of being resolved
by argument order. Its requested limit is hard-capped at 100 rows to keep
inspection cheap. `agent-delta-show --node-id <uuid>` requires a UUIDv4-shaped
protocol node id, emits the same protocol-safe JSON shape for exactly one
committed delta, and returns no result for ordinary memory nodes or unknown ids.
Missing or malformed node-id filters for show/edges are rejected before
workspace setup, so simple syntax mistakes do not create or touch a ledger.
Accepted node-id filters are normalized to canonical lowercase before lookup.
`agent-delta-stats` accepts the same bounded filters and emits aggregate
counts by outcome state, source, and review category, plus a review-required
count, without summaries, entities, node ids, raw text, hashes, vectors, scores,
or internal SQLite row ids. `agent-delta-summary` is the same aggregate view and
accepts the same filters. `agent-delta-edges` inspects bounded graph edges
touching committed agent deltas with protocol-facing source and target
`node_id` values, canonical relationship type, edge weight, and UTC edge
timestamps. Malformed legacy relationship labels are exported as `UNKNOWN`. It
accepts the same `--review-only`, `--review-category`, `--source`, `--entity`,
and `--state` filters as list/stats to first scope the committed delta nodes
locally, then applies graph-specific `--node-id <uuid>` and
`--relationship <type>` filters. The graph `--node-id` filter also requires a
UUIDv4-shaped protocol node id and is normalized to canonical lowercase before
lookup. Output is capped at 100 rows, relationship filter input is bounded and
normalized the same way, and malformed relationship filters are rejected before
workspace setup. It omits raw text, summaries, hashes, vectors, scores, and
internal SQLite row ids.
Malformed `--limit` values for list/stats-summary/edges are also rejected before
workspace setup, so syntax mistakes stay cheap and ledger-free. This is not a session watcher and it does not observe
browser/API activity automatically; it is the smallest explicit write-back
primitive for tested agent outcomes.

Compact inspection examples after a committed JSON write:

```powershell
cargo run -p identityd -- agent-delta-show --node-id <node_id-from-commit-json>
cargo run -p identityd -- agent-delta-edges --node-id <same-node-id> --relationship OUTCOME_FOR --limit 20
cargo run -p identityd -- agent-delta-edges --source billing --entity "Acme Capital" --state paid --review-category finance --relationship UPDATED_BY --limit 20
```

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

`context-now --project <name>` explicitly selects a named local project profile instead of relying on active-window matching. `start` is the simplest local context entrypoint. It runs the loopback capture server, idle-gated clean/promote pipeline, bounded foreground-window capture, and a global `Ctrl+Shift+I` hotkey that copies a compact sanitized context block to the clipboard. `start-identity.cmd` runs it visibly in the current terminal; closing that terminal also stops the daemon. `start-identity-hidden.cmd` starts the same default daemon in a hidden background process. `scripts\test-identity-hotkey.ps1` starts a temporary daemon, simulates `Ctrl+Shift+I`, verifies the clipboard receives an Identity context block, and restores the previous clipboard text. `scripts\test-identity-page-capture.ps1` starts a temporary loopback capture server on an ephemeral local port, verifies plain clipboard text is rejected by `capture-page --from-clipboard`, copies an `IDENTITY-PAGE-CAPTURE` envelope to the clipboard, runs `capture-page --from-clipboard --promote-now`, verifies the selected page memory is searchable, temporarily gives the test terminal a browser/agent-like title, and verifies `context-now --preview --project tfl-central` includes that selected page context in the temporary `.me` store. On Windows, default active-window capture records the foreground executable and title; set `IDENTITYD_ENABLE_DEEP_ACTIVE_WINDOW_TEXT=1` only for local debugging of deeper UI Automation/MSAA text extraction. `daemon` remains the lower-level phase 1/2 orchestration entrypoint: it runs the loopback capture server and idle-gated pipeline in one process, and it can optionally add a shutdown-aware filesystem watcher with `--watch-path`, bounded foreground-window capture with `--watch-active-window`, and hotkey capture with `--hotkey`. On Windows the filesystem watcher stays on the native event path. `--watch-path` uses the same safe-root policy as `watch`.
