# Vision Phase 1: Backpressure (Deferred)

This document captures the backpressure design that was originally part of vision-phase-1.md but deferred for later consideration.

## Backpressure

The kernel enforces per-call backpressure to avoid unbounded buffering.

Model: credit-based.

- Each call has two budgets: `bytes_credit` and `items_credit`.
- The kernel may emit:
  - `bytes` frames while `bytes_credit > 0`
  - `item`/`event`/`progress` frames while `items_credit > 0`
- When credit is exhausted, the kernel pauses emission until it receives an `ack`.

`ack` payload:
- `bytes`: u64 credits returned to the producer
- `items`: u64 credits returned to the producer

Defaults:
- The kernel may grant an initial credit window at `req` time (implementation-defined).
- The consumer should periodically `ack` as it renders/drains.

Stall safety:
- If a call is blocked on credit for longer than a stall timeout, the kernel may emit an `error` (`E_BACKPRESSURE_STALL`).

## Frame Ops (with backpressure)

If backpressure is adopted, add `ack` to request/control ops:

Request/control ops:
- `req`: start a call
- `cancel`: request cancellation of an in-flight call
- `ack`: consumer acknowledges progress to provide backpressure credits

Notes:
- `cancel` and `ack` are control-plane ops and should be accepted at any time after `req`.

## Observability (with backpressure)

Phase 1 should log:
- `req` received
- `ok`/`error` emitted
- cancellation events
- backpressure stalls
