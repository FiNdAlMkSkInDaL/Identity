# Engineering Roadmap: Identity

This roadmap sequences Identity from a local-first MVP into a broader protocol architecture.

The execution philosophy is to separate immediately viable local engineering from longer-term cryptographic research. Phases 1 and 2 should prove that a useful `.me` state bank can run on consumer hardware without cloud dependency. Phase 3 closes the feedback loop and begins protocol-scale security work.

## 1. Roadmap Principles

- Build local utility before protocol complexity.
- Prefer open-source embedded infrastructure where possible.
- Keep sensitive data on-device by default.
- Prove ingestion, retrieval, and context slicing before zero-knowledge R&D.
- Treat cryptographic and TEE features as scale-stage protocol work, not MVP dependencies.

## 2. Phase 1: Local-First Ingestion and Embedded Vector Database

Target window: Days 1-30

Objective: Establish the localized ingestion engine and prove that a `.me` database file can exist and update smoothly on a consumer device without cloud dependencies.

### Task 1.1: Local Host Core Application

- Build a desktop application using Tauri and Rust.
- Maintain a lightweight system footprint, targeting under 30MB idle RAM where practical.
- Establish the background daemon, `identityd`, to manage file structures, ingestion queues, and execution threads.

### Task 1.2: Database Implementation

- Embed LanceDB inside the runtime application process.
- Configure a single local encrypted directory path as the master storage ledger:

```text
~/.identity/identity.me
```

- Keep the storage layer local, embedded, and process-owned.

### Task 1.3: Ingestion Framework

- Implement local system monitoring through OS accessibility APIs:
  - macOS: `NSAccessibility`
  - Windows: `UI Automation`
- Capture raw active-window text strings and relevant app/window context.
- Deploy an optional local loopback network proxy on `127.0.0.1`.
- Convert captured page content into clean Markdown arrays by stripping DOM clutter, styling, ads, and tracker noise.

### Task 1.4: Edge Vectorization Pipeline

- Integrate ONNX Runtime for hardware acceleration.
- Support acceleration targets:
  - Apple: `CoreML`
  - Windows: `DirectML`
- Load a quantized lightweight embedding model:
  - `MiniLM-L6-v2`
  - `BGE-Micro-v2`
- Convert buffered Markdown into dense vectors on system-idle triggers.

## 3. Phase 2: Hotkey Context Injection Daemon

Target window: Days 31-60

Objective: Build the lightweight keyboard-driven interaction layer that captures local active window/control text on a global hotkey trigger, queries relevant memory, formats a compact context block, and copies it to the clipboard (or optionally pastes it) for direct AI tool consumption.

### Task 2.1: Context Snapshot and Active Window Capture
- Expose native active window metadata and focused-control text retrieval on Windows natively.
- Return structured window state containing process name, window title, focused text, and optional selected text.
- Enforce strict size bounds and sanitization.

### Task 2.2: Local Project Profile Matching
- Parse and load deterministic project profiles from `~/.identity/projects.json` or `projects.toml`.
- Match the active window title, process name, path, or domain to a specific project.
- Retrieve project-specific memory query terms and strict guardrails.

### Task 2.3: Context Builder and Budget Control
- Query the `.me` SQLite memory store using terms associated with the matched project.
- Package the matched guardrails, recent captures, active window environment, and semantic memory facts into a structured context block.
- Enforce a strict token/character size budget, trimming lower-ranked items first to ensure it remains pasteable (default max 8000 characters).

### Task 2.4: Clipboard and Native Hotkey Daemon
- Implement a global hotkey listener (`Ctrl+Space` or custom) natively using Win32 API.
- Copy generated context blocks to the system clipboard upon hotkey trigger.
- Optionally support native keyboard event simulation to paste the context block into the focused AI tool.

## 4. Phase 3: Bi-Directional State Synchronization and Protocol Scale

Target window: Days 61-90+

Objective: Close the operational data loop by tracking agent task outcomes and establish the groundwork for zero-knowledge remote enclave computing.

### Task 3.1: Headless Activity Recorder

- Program the Session Watcher Daemon inside the Rust network proxy layer.
- Log inbound API event streams, DOM modifications, and server responses during a task window.
- Scope recording to the active `.meslice` lifecycle.

### Task 3.2: Reverse Token Delta Extraction

- Route execution logs through the local SLM in reverse parsing mode.
- Extract structural state mutations, represented as deltas.
- Convert delta arrays into clean, valid JSON strings.

### Task 3.3: Graph Reconciliation and Decay

- Convert JSON delta variables into new nodes and structural edges inside the embedded vector graph.
- Implement edge-weight decay for older conflicting attributes:

```text
Weight_new = Weight_old * (1 - alpha)
```

- Prioritize recent user context during future semantic lookups without deleting useful history.

### Task 3.4: Trusted Execution Environment Integrations

- Transition from client-side injection to verified protocol streams for high-security endpoints.
- Build an infrastructure handshake that transmits encrypted `.meslice` payloads over ephemeral TLS directly to a secure hardware enclave.
- Evaluate AWS Nitro Enclaves, Intel SGX, and equivalent TEE environments.

## 5. LLM Subsystem Build Prompts

These prompts can be used as execution blocks for future AI-assisted implementation.

### Prompt 1: Local Capture and Buffer

```text
Write a Rust module using Tauri and a local loopback proxy architecture to catch HTTP streams, extract text data from HTML nodes, convert it into Markdown, and store it in an SQLite memory buffer database.
```

### Prompt 2: Embedded Vectorization Runtime

```text
Write a C++ or Rust implementation that bundles LanceDB inside an application folder, instantiates an ONNX Runtime environment leveraging Mac CoreML and Windows DirectML acceleration, and vectorizes buffered text strings using BGE-Micro-v2 on system idle state.
```

### Prompt 3: Local Context Boundary Pipeline

```text
Build a Python or JavaScript pipeline that reads an incoming user prompt, runs it through an SLM to identify needed context boundaries, queries a vector dataset for only those boundaries, and formats an ephemeral context-delimited string payload.
```

## 6. MVP Success Criteria

By the end of Phase 1:

- The app can write and update a local `.me` storage directory.
- Captured text can be buffered locally.
- Buffered text can be embedded locally.
- The system can retrieve semantically relevant memories without cloud storage.

By the end of Phase 2:

- The daemon runs with global hotkey support.
- A user can trigger a context capture in any editor, terminal, or browser window.
- The system generates a compact, sanitized context block and copies it to the clipboard.
- Project profiles dynamically match window metadata to inject specialized guardrails and retrieve scoped memory.

By the end of Phase 3:

- Agent execution outputs can be captured.
- A semantic delta can be extracted and validated.
- The `.me` graph can update from real task outcomes.
- Initial protocol-grade secure execution research is underway.
