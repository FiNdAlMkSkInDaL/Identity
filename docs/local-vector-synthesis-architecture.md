# Local Vector Synthesis Architecture: Sovereign

This document proposes the architecture for solving the first major Sovereign challenge: local vector synthesis via edge compute.

The goal is to eliminate cloud dependency by executing the complete data ingestion, synthesis, and vectorization pipeline directly on client hardware.

For implementation sequencing, see the Phase 1 plan in [Engineering Roadmap](engineering-roadmap.md).

Target operating constraints:

- Zero cloud compute costs.
- Sub-50MB memory footprint for ambient daemon operations.
- No noticeable CPU, GPU, or battery degradation during active user workflows.
- Privacy-preserving local processing by default.

## 1. System Topology

```text
[Ambient OS Activity]
        |
        v
(Rust Daemon Tracker)
        |
        v
[Local SQLite Transit Buffer]
        |
        |  System idle / NPU trigger
        v
[Local SLM Cleaner]
        |
        v
[Local Embedding Model]
        |
        v
[Hybrid Vector Graph DB]
```

The architecture separates capture from synthesis. Real-time listeners collect lightweight raw signals and write them into a local transit buffer. Heavier semantic cleaning and vectorization happen later, during idle windows or hardware-accelerated processing bursts.

## 2. Data Capture Layer

Sovereign should avoid fragile browser extensions or application-specific plugins as the primary capture path. Instead, it runs a lightweight background daemon written in Rust for memory safety, low overhead, and predictable execution.

### OS-Level Accessibility Hooks

The daemon hooks into native operating system accessibility APIs:

- Windows: `Windows UI Automation`
- macOS: `NSAccessibility`
- Linux: desktop-environment accessibility APIs where available

These APIs can pull raw text strings, active application context, window metadata, control hierarchy, and layout signals directly from the active interface.

### Local Loopback Network Proxy

Sovereign can provision a local loopback proxy on `127.0.0.1` for supported traffic flows.

Before raw HTML text is rendered inside a client browser, the proxy can intercept the stream, extract text nodes, strip styling and tracker noise, and transform DOM content into clean Markdown-like payloads for ingestion.

This should be treated as an optional ingestion path because TLS, browser security models, and user consent requirements make transparent interception sensitive.

### File System Listeners

The daemon watches approved local folders using kernel-level file event APIs:

- Linux: `inotify`
- macOS: `FSEvents`
- Windows: `ReadDirectoryChangesW`

When new text-bearing assets are created or modified, Sovereign queues them for ingestion in the local transit buffer.

## 3. Compute Layer

Data tokenization, cleaning, summarization, and embedding should happen locally using device hardware such as CPUs, integrated GPUs, discrete GPUs, or NPUs.

### Runtime Optimization Stack

The local runtime should support hardware-accelerated inference through:

- ONNX Runtime
- `ggml` or `llama.cpp` bindings
- CoreML on Apple Silicon
- DirectML on Windows devices
- WebGPU where browser or cross-platform acceleration is useful

The runtime should pick the best available backend per device and degrade gracefully to CPU-only execution.

### Dual-Model Cascade

The synthesis pipeline uses two model classes.

#### Filtering Layer

A quantized 1B-to-3B parameter small language model sanitizes captured text, removes semantic noise, and generates structured summaries.

Candidate model families:

- `Phi-3-mini-4k-instruct`
- `Gemma-2-2B`
- Similar local SLMs with permissive deployment characteristics

The output should be concise structured data: facts, preferences, entities, events, tasks, and candidate memory deltas.

#### Embedding Layer

A lightweight embedding model maps cleaned text chunks and structured summaries into dense vectors.

Candidate model families:

- `BGE-Micro-v2`
- `MiniLM-L6-v2`
- Similar compact embedding models optimized for local execution

The target is millisecond-scale embedding for small chunks during processing bursts.

### Smart Throttling Engine

To avoid degrading the user's active workflow, Sovereign should process queued data only under safe conditions.

The daemon monitors:

- CPU load
- GPU load where available
- Battery state
- Thermal pressure
- Foreground activity
- System idle duration

Captured text is first written to SQLite. When idle markers cross a threshold, such as 75% idle for more than 5 consecutive seconds, the daemon processes items in brief, controlled bursts.

## 4. Local Storage Layer

Traditional vector databases are strong at similarity search but weak at modeling chronological evolution and durable identity state. Sovereign needs a hybrid vector-graph architecture.

### Embedded Deployment Engine

The local `.me` storage layer should run embedded and in-process with no client-server networking overhead.

Candidate engines:

- LanceDB
- DuckDB with vector extensions
- SQLite with vector extensions for early prototypes

### Chronological Graph Architecture

The `.me` database should combine vector similarity with structured entity relationships.

Core shape:

```text
Entity -> Action -> Attribute
```

Old context strings should not be blindly deleted. Instead, the graph should decay relationship edge weights as newer interactions supersede older constraints.

For example, if a user used to prefer morning flights but repeatedly books evening flights, the system should reduce the confidence weight of the old preference rather than overwrite history destructively.

## 5. Operational Ingestion Flow

| Stage | Action | Execution Pipeline | State Latency |
| :--- | :--- | :--- | :--- |
| 1. Intercept | Capture clean page markup | Rust daemon / local loopback proxy | Real time, under 5ms |
| 2. Buffer | Commit raw strings into storage | Local SQLite cache DB | Instant, under 1ms |
| 3. Trigger | Wait for system idle check | OS telemetry watchdog | Asynchronous |
| 4. Purify | Condense strings into key data | Local 1B-3B SLM on NPU/GPU/CPU | Processing burst, under 1.5s |
| 5. Map | Calculate vector embeddings | Local embedding model | Processing burst, under 200ms |
| 6. Store | Append vectors and graph edges to disk | Local `.me` vector graph DB | Finalized, under 10ms |

## 6. Prototype Build Path

1. Build a Rust daemon that captures approved filesystem events and writes payloads into SQLite.
2. Add a local embedding path using a compact model and a simple vector store.
3. Add idle-aware throttling before processing queued items.
4. Introduce a local SLM cleaner for summarization and structured memory extraction.
5. Replace simple vector storage with a hybrid vector-graph `.me` store.
6. Add accessibility capture after the ingestion and storage loop is stable.
7. Treat loopback proxy capture as an advanced, opt-in feature after consent and security design are complete.
