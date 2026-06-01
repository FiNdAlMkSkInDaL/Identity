# Agent Constraints and Performance Budget: Identity

This document is a system-prompt supplement for automated coding agents working on Identity.

Identity runs on user edge hardware. Every kilobyte of memory overhead and every millisecond of latency matters. Agents must follow a subtractive engineering paradigm: do not write code for hypothetical future feature sets. Build the smallest low-level module required for the immediate implementation target.

## 1. Resource Budgets

Any architecture that requires a breach of these budgets should be rejected or brought back for human review.

| Budget | Limit |
| :--- | :--- |
| Production daemon binary, `identityd` | Under 15 MB |
| Idle background RAM | Under 35 MB |
| Peak ingestion RAM during local SLM bursts | Under 120 MB |
| Cold startup to OS hook binding | Under 150ms |

## 2. Dependency Policy

### Allowed Core Dependencies

Agents may use these foundational primitives when the task genuinely requires them:

- Async runtime: `tokio`, with features kept as narrow as possible.
- Embedded vector DB: `lancedb` and `arrow`.
- Local ingest buffer: `rusqlite`.
- Serialization: `serde` and `serde_json`.
- HTML stream parsing: `lol-html` only. Do not replace this with a full DOM scraper.

### Forbidden Dependencies

Do not introduce:

- Heavy HTTP frameworks such as `axum`, `actix-web`, or `rocket`.
- Embedded Node.js, V8, or JavaScript backend runtimes.
- Headless browser wrappers such as Playwright or Selenium inside the daemon.
- Full DOM scraping stacks for daemon ingestion.
- Hosted vector stores, cloud queues, or server-first ingestion infrastructure.

Loopback communication should use raw `tokio::net::TcpListener` sockets or a very small HTTP engine only if the codebase later proves raw sockets are insufficient.

## 3. Structural Coding Principles

### Build Lean

Every module should do one thing. Avoid generalized frameworks, large abstractions, speculative extension points, or feature flags for work not currently implemented.

### Minimize Allocation

Prefer references and bounded buffers over repeated cloning. Avoid converting between `String` and `Vec<u8>` unless ownership is necessary.

### Avoid Long-Lived Heap Churn

For high-frequency loops, prefer preallocated buffers, stack arrays, and reuse patterns. Avoid unbounded vectors, unbounded channels, and background tasks that can pile up work.

### Remove Dead Code

Delete uncalled enum variants, unused helpers, and speculative plumbing as soon as it becomes clear they are not needed.

## 4. Validation Routine

Before committing substantial daemon changes, agents should run or simulate:

```bash
cargo tree --duplicates
cargo test -p identityd
cargo build --release -p identityd
```

On Windows PowerShell, binary size can be checked with:

```powershell
(Get-Item target/release/identityd.exe).Length
```

The release binary must remain under 15 MB unless a human explicitly approves a temporary breach.
