# Cryptographic Context Fragmentation and Ephemeral Handshake Architecture: Identity

This document proposes the architecture for solving the second major Identity challenge: cryptographic context fragmentation and trustless context streaming.

The goal is to ensure that external AI agents, web applications, and foundational model APIs never interface directly with the master `.me` database. Instead, they receive isolated, time-bound context mutations called `.meslice` blocks.

For implementation sequencing, see the Phase 2 and Phase 3 plans in [Engineering Roadmap](engineering-roadmap.md).

## 1. Zero-Trust Access Paradigm

Identity treats all external AI agents, web applications, and foundational model APIs as untrusted environments that may scrape, retain, or ingest user data.

The ephemeral handshake architecture prevents third-party applications from touching the master `.me` database. They interact only with stateless, isolated, task-bound `.meslice` payloads that expire immediately after task completion.

```text
[External Agent Prompt]
        |
        v
(Identity Local Daemon)
        |
        |  Evaluates semantic boundary constraints
        v
[Generated .meslice]
        ^
        |
(Isolates Entities / Tokens)
        ^
        |
[Master .me Vector Graph]

[Generated .meslice]
        |
        +--> Route A: Client-side runtime injection via encrypted system prompts
        |
        +--> Route B: Remote secure hardware enclave execution via TEE / Intel SGX
```

## 2. Prompt Interception and Intent Parsing

Identity deploys a local proxy filter that hooks into standard machine transport layers. When an external autonomous worker or API makes an execution-loop request, the outbound payload is paused client-side.

### Semantic Extraction Pipeline

The intercepted raw agent prompt is passed through a fast local structural parsing model, likely in the 1B parameter class.

For example, if an agent submits:

```text
Draft a personalized follow-up pitch to investor David Lee regarding our seed deck.
```

The parser identifies the operational intent, target entity, relevant artifact, and required personalization scope.

### Need-to-Know Boundary Filter

The local system constructs an algorithmic boundary matrix. It isolates the exact variables required for the task while redacting out-of-bounds database trees.

Example whitelisted entities:

- `David Lee`
- `Seed Deck`
- `User's Core Pitch Value Proposition`

Example blacklisted entities:

- `User's Personal Calendar`
- `Unrelated Financial Ledgers`
- `Browsing History`
- `Private Messaging Logs with Third Parties`

## 3. Ephemeral Context Generation

Once request boundaries are mapped, the Identity local daemon executes a targeted semantic query against the embedded local database.

### Dynamic Mutation Engine

Instead of exposing raw data tables, the system reads only the requested nodes and constructs a temporary virtual memory block called a `.meslice`.

The `.meslice` is not the user's memory. It is a scoped mutation derived from memory for one execution path.

### Cryptographic Tokenization and Pseudonymization

Text fields inside the `.meslice` undergo dynamic masking.

Sensitive persistent records such as database record IDs, private phone numbers, explicit system identifiers, or durable account references are swapped for single-use, cryptographically salted session tokens.

These tokens map back to the real user profile only inside the local host loop. To an external API, they appear as random strings.

## 4. Zero-Knowledge Execution Pipeline

Identity supports two deployment pathways depending on the endpoint's capabilities.

### Pathway A: Client-Side Runtime Injection

For standard applications running through browser interfaces or open APIs, Identity intercepts the data transport layer and encapsulates the temporary `.meslice` payload inside cryptographic delimiters within the outbound system instructions.

Example context envelope:

```text
[IDENTITY-CONTEXT-BLOCK: ID_884920]
- Ephemeral tokenized context segment payload
- Authorization expiry signature: POSIX_TIMESTAMP + 2000MS
[IDENTITY-CONTEXT-BLOCK-END: ID_884920]
```

The client browser streams this combined package to the remote foundational model. When the task finishes or the expiry signature lapses, the local runtime closes the memory window and clears the temporary slice.

This pathway is practical for early integrations but cannot fully guarantee that a hostile endpoint will not retain data. It should therefore use the smallest possible `.meslice` and avoid sending sensitive raw values whenever tokenized substitutes can work.

### Pathway B: Hardware-Enforced Secure Enclaves

For high-security operations, Identity can use trusted execution environments through secure cloud hardware instances such as AWS Nitro Enclaves, Intel SGX, or equivalent audited TEE systems.

In this pathway:

1. The external application routes core processing instructions to an audited TEE instance.
2. Identity streams the encrypted `.meslice` payload over an ephemeral TLS connection terminated inside the secure hardware boundary.
3. The foundational model or task-specific runtime processes the data inside isolated encrypted memory.
4. The final output is returned to the client.
5. The enclave purges cryptographic keys and context state after execution.

This pathway is better suited to enterprise and protocol-compliant partners because it requires infrastructure support from the external execution environment.

## 5. Handshake Lifecycle

| State | Target Operation | Computational Location | State Security Profile |
| :--- | :--- | :--- | :--- |
| 1. Intercept | Catch outbound agent request | Local machine transport layer | Fully private / encrypted |
| 2. Parse | Isolate minimum required data nodes | Local NPU or local parsing SLM | Fully private / isolated |
| 3. Slice | Generate ephemeral `.meslice` block | Memory storage cache / RAM | Single-use virtual segment |
| 4. Execute | Process prompt inside LLM context window | Browser stream or remote TEE | Transit-encrypted handshake |
| 5. Terminate | Expiry signature reaches zero | System memory watchdog | Complete purge / zero cache |

## 6. Prototype Build Path

1. Define the `.meslice` envelope format, including metadata, expiry, scope, and token map references.
2. Build a local intent parser that extracts entities, task type, and required context categories from a prompt.
3. Implement a rule-based need-to-know boundary filter before introducing model-based policy inference.
4. Generate `.meslice` payloads from a mock `.me` graph using only whitelisted nodes.
5. Add pseudonymization and session-token mapping inside local memory.
6. Implement client-side runtime injection for a controlled local agent workflow.
7. Explore TEE execution only after the standard handshake semantics are stable.
