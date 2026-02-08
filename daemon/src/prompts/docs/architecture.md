# Architecture

Abbot is a multi-agent system built on a syscall-based kernel architecture.

## Core Components

- **Kernel**: Central message dispatcher. Routes frames between agents and syscall handlers.
- **Frames**: Typed messages (req, ok, error, item, event, done) that flow through the kernel.
- **Syscalls**: Named operations (e.g., `fs:read`, `llm:chat`) that agents invoke via frames.
- **Agents**: Head (decides), Hand (executes), Mind/Room (reflects).

## Concurrency Model

Work is assigned to lanes based on urgency:

- **Immediate**: Fast, stateless operations (e.g., `docs:list`, `fs:read`).
- **Need**: User-facing requests that require prompt attention.
- **Task**: Background work items delegated by heads to hands.
- **Room**: Long-running collaborative sessions (conclaves, scheduled reflections).

## Data Flow

1. User message arrives via chat adapter.
2. Head agent receives the message and reasons about next steps.
3. Head may invoke tools (syscalls) directly or delegate tasks to hands.
4. Hands execute tool sequences and report results back.
5. Mind/Room agents run periodic reflections on project state.
