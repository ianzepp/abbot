# Room Management Spec

## Overview

This document specifies a persistent, tick-driven room scheduling and execution system.

It replaces the current `mind:*` syscalls with a single `room:*` surface.

Rooms are used for Mind-like behaviors:

- `autonomy`: opportunistic, lightweight work proposal during idle
- `conclave`: deeper strategic reflection and proposals (rarer)

This spec intentionally removes "consult" as a first-class feature. It was aspirational and is not
implemented today.

## Goals

- Persistent scheduling (survives daemon restarts)
- Best-effort scheduling (precision is not required)
- A single execution surface: scheduled rooms run via `room:run`
- Clear introspection: list upcoming schedules and running rooms
- Remove `mind:*` syscalls (use `room:*` only)

## Non-Goals

- Exact timers (no strict cron semantics)
- Defining cancellation semantics for running rooms (TBD)
- Adding a consult mode

---

## Core Concepts

### Room Type

Supported `type` values:

- `autonomy`
- `conclave`

### Room Schedule Item

A schedule item is a durable record that states:

- what room type to run
- where (`scope`)
- not before when (`run_after_ms`)
- why (`reason`)

Schedule items are owned by the system and executed by a coordinator.

### Best-Effort Ticks

Execution is driven by a background coordinator that subscribes to ticks and checks for due
schedules periodically (e.g. every 60 seconds). Schedules are not expected to run at an exact
timestamp; they run "soon after" `run_after_ms`.

---

## Syscalls

### `room:schedule`

Create a new schedule item.

Request:

```json
{
  "type": "autonomy" | "conclave",
  "scope": "main",
  "run_after_ms": 1730000000000,
  "reason": "scheduled" | "boot" | "user_request" | "idle_policy",
  "wake_mode": "init" | "normal",
  "constraints": {
    "only_if_idle": true,
    "only_if_queues_empty": true
  },
  "context": "optional freeform context"
}
```

Response:

```json
{ "schedule_id": "<uuid>", "status": "scheduled" }
```

Notes:

- `run_after_ms` is a "not before" timestamp.
- `constraints` are advisory gating checks evaluated by the coordinator before it runs the room.

### `room:list`

List upcoming scheduled rooms and any currently running rooms.

Request:

```json
{
  "scope": "main",
  "status": ["scheduled", "running"],
  "type": ["autonomy", "conclave"],
  "limit": 50
}
```

Response:

- `op=item`: one schedule entry per item
- `op=ok`: summary with counts

Example schedule entry:

```json
{
  "schedule_id": "<uuid>",
  "type": "conclave",
  "scope": "main",
  "run_after_ms": 1730000000000,
  "status": "scheduled",
  "reason": "idle_policy",
  "wake_mode": "normal",
  "attempts": 0,
  "last_error": null
}
```

### `room:reschedule`

Move the `run_after_ms` timestamp for an existing schedule item.

Request:

```json
{
  "schedule_id": "<uuid>",
  "run_after_ms": 1730001111000,
  "reason": "user_request"
}
```

Response:

```json
{ "schedule_id": "<uuid>", "status": "scheduled", "run_after_ms": 1730001111000 }
```

Semantics:

- Only schedule items in `scheduled` state may be rescheduled.
- Rescheduling does not create a new schedule id.

### `room:cancel`

Cancel a scheduled room.

Request:

```json
{ "schedule_id": "<uuid>", "reason": "user_request" }
```

Response:

```json
{ "schedule_id": "<uuid>", "status": "cancelled" }
```

Notes:

- Cancellation semantics for `running` rooms are TBD and will be specified as part of the
  "how to run a room" process.

---

## Persistence

Schedules MUST be durable across daemon restarts.

Minimum persisted fields:

- `schedule_id` (uuid)
- `type` (`autonomy` | `conclave`)
- `scope`
- `run_after_ms`
- `status` (`scheduled` | `running` | `done` | `cancelled` | `failed`)
- `reason`
- `wake_mode`
- `constraints` (serialized)
- `context` (optional)
- `attempts` (integer)
- `last_error` (optional string)
- `room_id` (optional uuid; set when execution starts)
- `created_at_ms`, `started_at_ms`, `finished_at_ms`

---

## Coordinator

### RoomCoordinator

A single background service is responsible for running due schedules.

High-level behavior:

- Subscribes to `tick:subscribe`.
- On each check interval (e.g. every 60 seconds):
  - List due schedules: `status == scheduled` and `run_after_ms <= now`.
  - Apply constraints (idle/queues) as gating checks.
  - Claim each schedule item atomically (`scheduled -> running`).
  - Execute by calling `room:run`.
  - Mark the schedule item `done` or `failed`.

Correctness requirements:

- The claim step MUST be atomic to prevent double-run.
- If a run fails, `attempts` increments and the item becomes `failed` (or remains scheduled if
  retry is desired; retry policy is a separate decision).

---

## Execution Surface

Scheduled rooms MUST run via `room:run`.

The exact "how to run a room" protocol (streaming, transcript capture, cancellation, retries) is
intentionally not specified here and will be covered in a dedicated execution spec.

---

## Migration Notes

- Delete `mind:*` syscalls (`mind:autonomy`, `mind:conclave`) once `room:*` scheduling and
  execution is in place.
- Replace `MindService` scheduling logic with schedule creation (or remove `MindService` entirely
  if all scheduling is expressed through `room:schedule`).
