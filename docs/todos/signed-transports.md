# Signed Transports (Rough Plan)

Goal: allow Abbots (each a collective) to communicate locally and non-locally over multiple external transports (email, Slack, Discord, etc) while cleanly separating human traffic from machine-to-machine protocol traffic.

This plan treats the existing in-process bus as intra-process infrastructure only, and adds a transport boundary at the edge via a "secretary" runtime service.

## Core Idea

- Keep `crate::bus` as the domain protocol for in-process coordination (scopes, Need/Task lifecycles, UI streaming).
- Add a "Group Secretary" runtime service that bridges external transports into the local bus.
- External messages are classified as either:
  - Human traffic (free-form), or
  - Abbot protocol traffic (structured + cryptographically signed).

## Why Signed Protocol Traffic

- Email/chat transports are noisy and forgeable.
- The secretary needs a reliable way to:
  - authenticate which peer Abbot produced a message,
  - reject/ignore spoofed machine messages,
  - still accept human messages as inputs (but with different handling),
  - generalize across transports without coupling core logic to any transport.

## High-Level Architecture

### Secretary Service (Edge)

Responsibilities:

- Ingress:
  - Poll/watch external transport inboxes.
  - Parse incoming items into either:
    - `InboundHumanMessage`, or
    - `InboundSignedEnvelope` (verified).
  - Deduplicate (by message id) and persist cursors/state.
  - Publish to the internal bus in a dedicated scope (e.g. `#group/<group_id>`).
- Egress:
  - Watch internal bus for outbound intents (e.g. `NeedMsg::Request` with a recognizable `source`, or `Event kind="group_send_request"`).
  - Materialize an outbox entry, then create a Hand task to send via the chosen transport.
  - Publish success/failure audit events back to the bus.

### Internal Bus Contract

- The internal bus remains in-process fanout (fast), with SQLite as an audit/event log.
- Cross-process reliability is handled by secretary-level inbox/outbox durability, not by extending the internal bus itself.

## Transport-Agnostic Envelope

Define a canonical, transport-agnostic JSON envelope for Abbot protocol messages.

Suggested fields:

- `schema_version`: integer
- `group_id`: string
- `message_id`: string (globally unique; UUID is fine)
- `from_node`: string (stable node id)
- `timestamp_ms`: integer
- `kind`: string (e.g. `conclave_update`, `proposal`, `need_offer`)
- `body`: object (payload)
- `sig`: object
  - `alg`: string
  - `key_id`: string (fingerprint / key reference)
  - `signature`: string (base64)

Canonicalization:

- Define a stable canonical byte representation for signing and verifying.
- Do not sign transport-specific wrapper fields.

## Identity and Signing

### Identity Requirements

- Each Abbot has a long-lived identity keypair.
- Abbot exports a public key and stable `node_id`.
- Verification is per-group:
  - Group config contains an allowlist of peer identities, or
  - A group root key signs member keys (optional).

### GPG vs Ed25519

- Email-centric approach: GPG keys are socially legible and integrate with mail tooling.
- Cross-transport approach: Ed25519 is simpler to embed, canonicalize, and verify.

Hybrid approach:

- Maintain an internal Ed25519 identity for federation envelopes.
- Optionally publish a GPG key for email and bind it by cross-signing (GPG signs Ed25519 public key, or vice versa).

## Human vs Protocol Classification

Inbound classification logic should be transport-specific, but the outcomes should not be.

- If a message contains a valid signed envelope from an allowed peer: treat as Abbot protocol.
- Otherwise: treat as human traffic.

Human traffic handling:

- Convert to a `NeedMsg::Request` with `source = "human:<transport>"`.
- Preserve enough metadata to respond on the same transport (reply address/channel id).

Protocol traffic handling:

- Publish as `MessageOp::Event kind="federation_inbound"` (or a dedicated op), in `#group/<group_id>`.
- Include verified peer identity (`from_node`, `key_id`) in the payload.

## Inbox/Outbox Durability

To survive process restarts and avoid duplicates:

- Inbox dedupe table:
  - `(group_id, message_id) PRIMARY KEY`
  - `seen_at_ms`, plus optional transport receipt ids.
- Outbox table:
  - `id`, `group_id`, `kind`, `payload`, `state` (`queued|sending|sent|failed`), `last_error`, timestamps.
  - The secretary can retry failed sends with backoff.

## Transports

Transport implementations are pluggable behind a small interface:

- `poll_inbound(group_id) -> Vec<InboundItem>`
- `send(group_id, OutboundItem) -> SendReceipt`

Email transport notes:

- Mailing list address acts as broadcast.
- Each Abbot should receive its own copy (do not share a single mailbox if every Abbot must see every message).
- Store and use Message-ID for dedupe.

Chat transports notes (Slack/Discord):

- Use a predictable message wrapper for signed envelopes (e.g. fenced JSON block, or attachment payload).
- Avoid leaking secrets; only public keys and signatures are required.

## Suggested First Milestone

- Add secretary service skeleton that:
  - publishes inbound "human" items as needs,
  - publishes inbound "protocol" items as verified events,
  - writes dedupe state.
- Implement a single transport end-to-end (email or local file/Maildir) with signing + verification.
- Add a minimal "send request" pathway so conclave can produce an outbound update via the secretary.
