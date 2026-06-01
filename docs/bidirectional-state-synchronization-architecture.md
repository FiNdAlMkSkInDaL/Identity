# Bi-Directional State Synchronization and Feedback Loop Architecture: Identity

This document proposes the architecture for solving the third major Identity challenge: capturing the outputs and side effects of agent execution and safely writing them back into the local `.me` state bank.

The goal is to make the `.me` file continuously evolve with the user's real-world actions while avoiding platform-owned memory, recursive ingestion loops, duplicate state, and vector bloat.

For implementation sequencing, see the Phase 3 plan in [Engineering Roadmap](engineering-roadmap.md).

## 1. Closed-Loop Synchronization Paradigm

Identity operates on a closed-loop data architecture.

The ultra-abstracted agentic web is often stateless, asynchronous, and background-driven. An agent may book travel, update a CRM, sign a document, send a message, or modify account settings without the user manually navigating a GUI.

Identity must therefore capture operational outputs, mutations, confirmations, and side effects from each agent execution block. These changes are routed safely back to the user's host machine, where the master `.me` database is updated locally.

The feedback loop ensures that durable memory belongs to the user, not the remote platform.

## 2. Headless Activity Recorder

Identity bundles every outbound ephemeral `.meslice` handshake with an isolated, low-overhead client-side monitoring loop called the Session Watcher Daemon.

The watcher observes the session only inside the scope authorized for the active task. It should not become general surveillance infrastructure.

### Session Watcher Daemon

The Session Watcher Daemon operates through:

- Local loopback transport hooks
- Browser or API request observers where authorized
- OS automation runner hooks
- Session-scoped memory buffers
- `.meslice` lifecycle metadata

It starts when a task-bound `.meslice` is generated and terminates when the session completes, expires, or is manually revoked.

### Payload Shadowing Engine

The daemon shadows network communications and agent actions that occur during an active session block.

When a background agent alters a DOM structure, issues an API `POST` request, receives a confirmation token, or completes a transaction, Identity mirrors the relevant raw JSON, HTML, or protocol response into a temporary local memory pipe before the remote session terminates.

Examples of captured outputs:

- Airline booking confirmations
- CRM update responses
- Contract signature receipts
- Calendar event mutations
- Payment or invoice metadata
- Message send confirmations

The raw shadow log is volatile. It is not committed directly to the master `.me` database.

## 3. Semantic Delta Extractor

Raw logs, telemetry noise, repeated HTML strings, and transport metadata cannot be committed directly to `.me` without causing vector bloat and memory corruption.

Identity therefore pipes the captured shadow log into a local extraction pipeline that computes the precise semantic state delta.

### Reverse Token Synthesis

The local 1B-to-3B parameter SLM runs in structured reverse-ingestion mode.

Instead of compressing local context for outbound use, it parses the execution log and extracts the meaningful state mutation:

```text
Execution log -> Durable semantic delta
```

The model discards transactional boilerplate, interface noise, duplicate strings, tracking parameters, and low-value telemetry.

### Structural Variable Conversion

The extractor converts meaningful task outputs into structured JSON blocks.

Example:

```json
{
  "transaction_state": "SUCCESS",
  "protocol_layer": "Headless_Browser_Session_9921",
  "mutated_entities": [
    {
      "entity_type": "ORGANIZATION",
      "entity_name": "British Airways",
      "relationship_type": "BOOKED_TRAVEL"
    }
  ],
  "associated_metadata": {
    "booking_reference": "BA99X2",
    "departure_timestamp": "2026-06-15T08:30:00Z",
    "seat_assignment": "12C"
  }
}
```

This structure becomes the candidate memory delta. It should still pass validation before being merged into `.me`.

## 4. Graph Reconciliation and Edge-Weight Decay

Once the structural delta is isolated, it must be merged into the master vector-graph database without destructively overwriting user history.

Identity uses a non-destructive, time-aware graph synchronization strategy.

### Deterministic Node Insertion and Schema Merging

The structured JSON payload is converted into:

- Entity nodes
- Relationship edges
- Attribute records
- Timestamped event records
- Vector embeddings for semantic retrieval

These are written into the local embedded storage engine, such as LanceDB, DuckDB with vector extensions, or an early SQLite-based prototype.

### Conflict Detection

Before insertion, the reconciliation layer checks whether the delta conflicts with existing state.

Examples:

- A changed travel preference
- A new primary email address
- A replaced company role
- A modified software stack
- A rescheduled meeting

Conflicts should produce new time-aware edges rather than deleting older records.

### Mathematical Edge-Weight Decay

To track changing human preferences and relationships, Identity avoids static overrides.

When a conflicting state update occurs, the system applies a decay coefficient to older graph edges:

```text
Weight_new = Weight_old * (1 - alpha)
```

This suppresses outdated nodes during semantic retrieval while preserving historical context.

Subsequent `.meslice` generation automatically prioritizes the strongest and most current behavioral signals.

## 5. Feedback State Synchronization Matrix

| Phase | Operation Target | Lower-Level Mechanism | Memory State Stability |
| :--- | :--- | :--- | :--- |
| 1. Shadow | Intercept background agent outputs | Session Watcher Daemon network hooks | Volatile memory buffer |
| 2. Extract | Parse log arrays into clean variables | Local 1B-3B SLM engine run | Volatile memory buffer |
| 3. Reconcile | Merge new vectors into local schema | LanceDB / DuckDB input array | Writing to local disk |
| 4. Decay | Attenuate obsolete relationship edges | Chronological graph weight adjustments | Persisted protocol update |

## 6. Safety Constraints

The feedback loop is powerful and must be constrained carefully.

- Session watchers must be task-scoped and expire with the `.meslice`.
- Raw shadow logs should remain volatile unless explicitly promoted.
- Memory deltas should be validated before insertion.
- Duplicate detection should run before vectorization.
- User-confirmation gates should exist for sensitive categories such as finance, health, legal identity, and private communications.
- The system should preserve historical state while allowing newer behavior to dominate retrieval.

## 7. Prototype Build Path

1. Add a session-scoped watcher around a controlled local agent workflow.
2. Capture mock API responses and browser-like action logs into a volatile buffer.
3. Build a deterministic extractor for known transaction types before using an SLM.
4. Convert extracted outputs into structured memory delta JSON.
5. Validate deltas against a simple `.me` schema.
6. Write deltas into a local vector-graph prototype.
7. Add edge-weight decay for conflicting preferences and time-sensitive facts.
8. Add user review gates for sensitive memory updates.

## 8. Strategic Importance

This feedback loop is what makes Identity more than a private context vault.

Without synchronization, `.me` is a static archive. With synchronization, it becomes a living local state bank that evolves from real actions across the web while keeping durable memory under user control.
