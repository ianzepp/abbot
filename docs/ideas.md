# Ideas

## Minds as a role
- Treat "minds" as a first-class role concept

## Rooms as multi-LLM meeting spaces
- Rooms become general-purpose meeting spaces for multiple LLMs
- Not tied to a single model or provider

## Update checking
- Add update/version checking to the TUI and web UI

## Provider/model registry
- Hardcoded providers/models in the project vs external JSON fetch
- Trade-offs: simplicity and offline use vs staying current without releases

## Separate release binaries
- abbot-releases ships separate binaries: abbot, abbot-tui, abbot-web

## Team syscall vs room concept
- Evaluate whether team:* belongs as a syscall frame or maps better to rooms

## Additional communication channels
- Mail, text message, etc. as input/output channels for abbot

## Goal as a parent concept
- "Goal" as an optional parent/grouping for need and task
- Consider a new "todo" type, or just treat it as a future-dated task

## Dev personality distillation
- Analyze 3000+ personal chat transcripts from last 6 months
- Distill a "dev personality" profile that defines how abbot should approach project work

## Syscall elapsed time tracking
- Kernel tracks elapsed times for each syscall
- Enables future reporting and performance optimization

## Rework memory syscalls
- Clarify purposes of self/ltm/stm
- Option A: dedicated syscalls with clearer names
- Option B: merged "recall" syscall with a type field

## Model autoselection
- Automatically select model based on task requirements

## Fire and forget tasks
- Tasks that can be dispatched without waiting for completion

## Explicit "explore" tool call
- Dedicated internal tool for open-ended exploration/research

## Frame logging to files
- Log frames out to files for debugging/audit
