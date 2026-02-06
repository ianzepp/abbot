# Room Management Spec

## Overview

This document specifies a persistent, tick-driven room scheduling and execution system.

It replaces the current `mind:*` syscalls and the structured conclave/autonomy deliberation engine
with a single `room:*` surface backed by a generic multi-agent execution loop.

Rooms are the universal orchestration primitive. A room is a shared context where multiple
LLM-backed participants work together — talking, proposing, voting, and executing real work — driven
by the coordinator in rounds until all participants are done.

What makes a room a "conclave" vs a "work session" is not the engine — it's the **tool set** and
**room prompt** given to participants. The execution model is always the same.

## Goals

- Persistent scheduling (survives daemon restarts)
- Best-effort scheduling (precision is not required)
- A single execution surface: scheduled rooms run via `room:run`
- Clear introspection: list upcoming schedules and running rooms
- Remove `mind:*` syscalls (use `room:*` only)
- Replace the structured grammar/vote engine with tool-based participant actions
- Support both deliberation and real work (code exploration, editing, testing) in the same model

## Non-Goals

- Exact timers (no strict cron semantics)
- Adding a consult mode

---

## Core Concepts

### Room Type

Supported `type` values:

- `autonomy`: opportunistic, lightweight work proposal during idle
- `conclave`: deeper strategic reflection and proposals (rarer)
- `work`: collaborative task execution (e.g. working a GH issue in a shared worktree)

Room types are not hard-coded behaviors — they are configurations that determine the room prompt,
the participant set, and the available tool set.

### Room Prompt

Each room type has an associated **room prompt** that defines the purpose and context of the room.
The room prompt is shared with all participants at the start. It replaces the structured grammar
that previously forced participants into a fixed JSON output format.

The room prompt describes:

- What kind of work the room is for
- What context is available (needs queue, LTM, recent activity, issue details, etc.)
- What tools participants have access to and when to use them

Participants converse in free-form text and take actions via tool calls.

### Participants

Each room has one or more **participants** — LLM-backed personas that are called each round. The
room type determines which participants are included.

A room may have a single participant (e.g. one agent working a GH issue solo) or multiple
participants (e.g. three minds deliberating in a conclave). The execution model is the same
regardless of count — a single-participant room is just a room where one agent works in rounds
with tool calls until it calls `room__done`.

Each participant has:

- A name and persona (system prompt)
- A temperature setting
- Access to the room's tool set

Participants are called **in parallel** each round. Each participant receives:

- The room prompt
- The accumulated room transcript (all prior rounds, including other participants' tool call results)
- The room's tool set

### Tool Sets

The room's tool set defines what participants can do. Tools fall into two categories:

#### Coordination Tools (available in all rooms)

These tools manage the room's decision-making process:

- `room__propose` — submit a typed proposal
- `room__vote` — cast a vote on a proposal
- `room__done` — signal this participant is finished

#### Work Tools (available per room type)

These tools perform real work. The coordinator executes them, blocks until results are available,
and feeds the results back into the room transcript for all participants to see.

Examples:

- `hand__explore` — search/read the codebase
- `hand__edit` — modify files in the shared worktree
- `hand__test` — run tests
- `hand__commit` — commit changes
- `hand__shell` — execute a shell command

Work tools are what make a room a workspace, not just a conversation. A conclave room might only
have coordination tools. A work room has coordination tools plus work tools.

The tool set is determined by the room type configuration and is fixed for the lifetime of the room.

### Tool Execution Model

Every tool is classified as either **readonly** or **mutating**:

- **Readonly**: `hand__explore`, `hand__test` (read-only test runs), `room__vote`, `room__done`
- **Mutating**: `hand__edit`, `hand__commit`, `hand__shell`, `room__propose`

When the coordinator collects tool calls from a round, it executes them in two phases:

1. **Readonly phase**: All readonly tool calls from all participants are executed **in parallel**.
   This is safe because they cannot interfere with each other.

2. **Mutating phase**: All mutating tool calls are executed **serially**, in the order they were
   received. This prevents conflicts (e.g. two participants editing the same file).

Coordination tools (`room__*`) are always processed by the coordinator itself inline. Work tools
(`hand__*`) are dispatched to the underlying system and blocked on.

If any tool call fails, the failure is reported back to the room transcript as a tool result. The
room continues — participants see the error and can adapt.

All tool results are part of the shared transcript. When participant A calls `hand__explore` and
gets results, participant B sees those results in the next round's context.

### Worktrees

Rooms that perform real work (type `work`) are closely tied to **git worktrees**. When a work room
is created, the coordinator provisions a dedicated worktree for it. This gives the room an isolated
filesystem workspace without branch conflicts:

- The worktree is created from the target branch (e.g. `git worktree add /work/<room_id> -b room/<room_id>`).
- All `hand__*` work tools operate within the room's worktree.
- Participants see the worktree path in the room prompt.
- When the room ends, the worktree can be cleaned up or left for review depending on outcome.

Worktrees are not mandatory — conclave and autonomy rooms do not need them because they only
deliberate. But for any room that writes code, the worktree is the natural isolation boundary.

---

## Coordination Tools

### `room__propose`

Submit a typed proposal for the room to consider.

```json
{
  "kind": "need" | "want" | "ltm" | "self" | "control",
  "content": "description of the proposal",
  "metadata": { }
}
```

The `metadata` field carries kind-specific data:

- `need`: `{ "reconvene": true | false }`
- `want`: `{}`
- `ltm`: `{ "op": "append" | "replace" | "remove", "pattern": "optional match pattern" }`
- `self`: `{ "op": "append" | "replace" | "remove", "pattern": "optional match pattern" }`
- `control`: `{ "mode": "reboot_collective" }`

Returns a `proposal_id` that other participants can reference when voting.

### `room__vote`

Cast a vote on an existing proposal.

```json
{
  "proposal_id": "<id>",
  "vote": "yes" | "no",
  "reason": "optional justification"
}
```

A proposal passes when it reaches the vote threshold (default: 2/3 of participants). The proposer
implicitly votes "yes" on their own proposal.

### `room__done`

Signal that this participant has nothing more to contribute.

```json
{}
```

Once a participant calls `room__done`, they are not called in subsequent rounds.

---

## Room Lifecycle

1. **Setup**: Coordinator creates the room, assembles the room prompt and initial context,
   determines the participant set and tool set from the room type configuration.

2. **Rounds**: Each round, the coordinator calls all active participants in parallel. Each
   participant receives the room prompt and accumulated transcript, and may respond with free-form
   text and/or tool calls.

3. **Tool Resolution**: The coordinator processes all tool calls from the round:
   - Coordination tools are handled inline (proposals recorded, votes tallied, done flags set).
   - Work tools are dispatched and blocked on; results are appended to the transcript.

4. **Accumulation**: All responses and tool results are appended to the shared transcript.

5. **Termination**: The room ends when:
   - All participants have called `room__done`, OR
   - The maximum round count is reached (safety valve)

6. **Resolution**: The coordinator tallies votes on all proposals and executes those that passed
   the vote threshold.

---

## Room Schedule Item

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
  "type": "autonomy" | "conclave" | "work",
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
  "type": ["autonomy", "conclave", "work"],
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

---

## Persistence

Schedules MUST be durable across daemon restarts.

Minimum persisted fields:

- `schedule_id` (uuid)
- `type` (`autonomy` | `conclave` | `work`)
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

### `room:run`

When the coordinator (or any caller) invokes `room:run`, the following happens:

1. **Load room type config**: Determine the room prompt template, participant set, tool set, and
   max_rounds from the room type.

2. **Assemble room prompt**: Inject context into the prompt template (needs queue state, LTM,
   recent activity, scope info, wake mode context, issue details for work rooms, etc.).

3. **Initialize participants**: Create the participant set. Each participant gets a persona (name,
   system prompt, temperature) and the room's tool set.

4. **Run rounds**: For each round up to `max_rounds`:
   - Call all active participants **in parallel**, passing the room prompt and accumulated
     transcript plus the tool set.
   - For each participant response, process tool calls in order:
     - `room__*` tools: handle inline (record proposal, tally vote, mark done).
     - `hand__*` tools: dispatch, block until complete, append result to transcript.
   - If all participants have called `room__done`, stop early.

5. **Resolve proposals**: After the final round:
   - Tally votes on all proposals. A proposal passes with >= 2/3 participant votes.
   - The proposer implicitly votes "yes".
   - Execute passing proposals by type (enqueue needs, apply LTM ops, etc.).

6. **Emit result**: Return the room decision (list of executed proposals) and final transcript.

---

## Examples

### Single-Agent Work Room

A solo `work` room for issue #42:

- **Worktree**: `/work/<room_id>` branched from `main`.
- **Room prompt**: "You are working on GH issue #42: 'Fix login timeout'. Your worktree is at
  /work/<room_id>. Explore the code, implement a fix, run tests, and propose a commit when done."
- **Participants**: One work agent.
- **Tool set**: `room__propose`, `room__done`, `hand__explore`, `hand__edit`, `hand__test`,
  `hand__commit`.

- **Flow**:
  1. Round 1: Agent explores the codebase. Reads relevant files.
  2. Round 2: Agent edits the fix. Runs tests. Tests fail.
  3. Round 3: Agent fixes the test failure. Runs tests again. Tests pass.
  4. Round 4: Agent proposes a commit and calls `room__done`.
  5. Resolution: Commit proposal auto-passes (sole participant); coordinator executes it.

No voting needed — with one participant, any proposal automatically meets the threshold.

### Multi-Agent Work Room

A collaborative `work` room for issue #42:

- **Worktree**: `/work/<room_id>` branched from `main`.
- **Room prompt**: "You are working on GH issue #42: 'Fix login timeout'. The issue describes...
  Your shared worktree is at /work/<room_id>. Use hand__explore to understand the code,
  hand__edit to make changes, hand__test to verify. When you believe the fix is complete,
  use room__propose to propose it for review."
- **Participants**: Two work agents (e.g. one investigator, one implementer).
- **Tool set**: `room__propose`, `room__vote`, `room__done`, `hand__explore`, `hand__edit`,
  `hand__test`, `hand__commit`.

- **Flow**:
  1. Round 1: Both participants call hand__explore in parallel (readonly — executed concurrently).
     Results go into transcript.
  2. Round 2: Participant A proposes a fix approach. Participant B sees A's exploration results
     and calls hand__edit (mutating — executed serially).
  3. Round 3: Participant B calls hand__test, sees failures, calls hand__edit again.
     Participant A votes on the approach and explores related code.
  4. Round 4: Tests pass. Participant B proposes a commit. Both vote yes and call `room__done`.
  5. Resolution: The commit proposal passes (2/2 votes); coordinator executes it.

---

## Future: Participant Allocation

> This section is aspirational. It captures the shape of the problem without specifying a solution.

Room participants are not generic — they map to the system's existing role taxonomy:

- **Minds**: thinking only (deliberation, proposals, voting). No work tools.
- **Heads**: read and write (can explore and edit code). Full work tool access.
- **Hands**: read only (can explore but not mutate). Readonly work tools only.

This has resource implications. When a head is allocated to a room, it is unavailable for other
work (e.g. processing the needs queue). The `room:schedule` request will need to express participant
requirements so the coordinator can:

- Reserve the right participant types for the room.
- Avoid starving other work by over-allocating heads to rooms.
- Potentially choose different LLM models per participant role (e.g. a cheaper model for hands
  doing exploration, a more capable model for heads doing implementation).

The eventual API shape might look like:

```json
{
  "participants": [
    { "role": "mind", "persona": "MindManager" },
    { "role": "head", "persona": "HeadManager", "model": "default" },
    { "role": "hand", "persona": "explorer", "model": "fast" }
  ]
}
```

Open questions:

- How does the coordinator decide whether to grant a head to a room vs keep it available?
- Can participant allocation be dynamic (room starts with a mind, escalates to request a head)?
- Should rooms be able to release participants mid-session (e.g. hand finishes exploration,
  becomes available for other work while the head continues)?

---

## Migration Notes

- Delete `mind:*` syscalls (`mind:autonomy`, `mind:conclave`) once `room:*` scheduling and
  execution is in place.
- Replace `MindService` scheduling logic with schedule creation (or remove `MindService` entirely
  if all scheduling is expressed through `room:schedule`).
- Remove the structured grammar-based deliberation in `Conclave` (`convene_with_trace`,
  `autonomy_with_trace`, `query_mind_with_grammar`). The room prompt + tool calls replace the
  grammar.
- The multi-round voting logic moves from `Conclave::tally_votes` into the coordinator's proposal
  resolution step.
- Mind personas (MindManager, HeadManager, HandManager) become room participants with the same
  names, system prompts, and temperature settings.
- The `hand:*` syscalls become work tools available to rooms, not just direct syscalls.
