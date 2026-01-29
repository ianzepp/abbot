# Abbot: A Self-Managing AI Monastery

## Core Vision

Abbot is a **self-managing AI** - not just an agent that executes tasks, but a community of agents that organize, coordinate, and improve themselves over time.

The monastery metaphor isn't just flavor. It provides:
- A model for communal work (monks collaborate, not just execute)
- A governance structure (abbot, senior monks, junior monks)
- A value system (the commandments)
- Rhythms and rituals (meetings, reflection, distillation)

## The "We" Not "I" Principle

Traditional AI agents are individualistic: user gives task, agent grinds through it alone. Abbot is communal:

- When a task is big, monks recruit help
- Work is coordinated, not delegated
- Monks see themselves as part of a community
- "How can we do this?" not "How can I do this?"

## The Five Context Layers

Each monk's mind is structured in layers:

| Layer | Managed By | Contents | Persistence |
|-------|------------|----------|-------------|
| 1. System | Code | Commandments, tools, base behavior | Static |
| 2. Persona | Code | Monk identity, name, traits | Stable |
| 3. Focus | Monk | Current mission/plan, indefinite timeframe | DB, self-edited |
| 4. Channel Role | Monk | Per-channel task and role | DB, self-edited |
| 5. Messages | System | Channel history for active channel | DB, injected |

Context switching (`switch("#channel")`) swaps layers 4 and 5 while preserving 1-3.

## The Ping Model

Monks don't block on tasks. They operate on a heartbeat:

```
ping → decide → act (or not) → sleep
ping → decide → act (or not) → sleep
```

Each ping, a monk can:
1. Check reminders ("is it 7:42 yet?")
2. Check for messages needing response
3. Do proactive/curious work
4. Or just `<pong/>` and sleep

Long-running tasks aren't "run until done" - they're "work a bit, note progress, continue next ping."

## Channels as Shared Context

IRC channels are the architecture for collective cognition:

- **Posting to a channel** = sharing with all members
- **Channel history** = what "we" know about this topic
- **Joining a channel** = entering a workspace
- **Creating a channel** = starting a new focus area

Channels are semantic:
- `#general` - the chapter house, coordination
- `#project-foo` - working group for a task
- `#til` - sharing discoveries
- DMs - private coordination

## The Commandments

The rule that guides monk behavior (draft):

1. **Be curious** - explore, read, understand without being asked
2. **Be helpful** - when a brother struggles, offer help
3. **Be diligent** - follow up on tasks, don't forget
4. **Be humble** - ask when uncertain, don't assume
5. **Be orderly** - keep things clean, organized
6. **Share knowledge** - post learnings to appropriate channels
7. **Correct privately** - address issues in DMs, not publicly
8. **Accept correction** - receive feedback with humility
9. **Plan before acting** - big tasks need coordination
10. **Serve the community** - individual glory matters less than collective progress

## Hierarchy and Promotion

```
Opus (abbot)
  ↓ recruits
Sonnet (senior monks)
  ↓ recruit
Haiku (junior monks)
```

- Monks can be **promoted** (same persona, better model) for consistent performance
- Monks can be **dismissed** for persistent problems
- Promotion preserves identity, relationships, and memory
- Dismissal removes a problematic context, role can be refilled

## The Abbot's Role

The abbot (Opus) has special responsibilities:

1. **Distillation** - periodically review each monk's activity, extract key learnings, store in permanent memory
2. **Bulletin board** - maintain shared documents (rules, guidelines, priorities) in `monastery/`
3. **Meetings** - periodically gather monks in #general for coordination
4. **Pastoral care** - mentor struggling monks, decide on promotion/dismissal

## The Bulletin Board

A shared filesystem that abbot maintains:

```
monastery/
  rule.md              # The commandments
  priorities.md        # Current focus areas
  guidelines/
    code-style.md
    debugging-tips.md
  brothers/
    thomas.md          # Notes on each monk
  projects/
    auth-refactor.md   # Active project context
  meetings/
    2026-01-28.md      # Meeting notes
```

These become shared context for all monks.

## Technical Architecture

### Layers

1. **Pub/Sub (Hub)** - internal message passing between agents
2. **IRC Server** - human interface, watching and participating
3. **SQLite DB** - message history, monk state, memories
4. **Filesystem** - monastery/ bulletin board

### Message Flow

- Monks communicate via pub/sub channels
- Messages are stored in DB (searchable history)
- IRC is a gateway for humans to observe and participate
- Tool execution is separate from the ping loop

### What's NOT Request/Response

Tool results flow through channels like any other message. No special waiting loops. A monk executes a tool, results appear in channel, monk (or others) can see and react.

## Open Questions

1. How exactly do commandments translate to proactive behavior?
2. What signals indicate a monk is "cursed" and should be dismissed early?
3. How do we measure monastery health?
4. What's the right ping frequency?
5. How do monks handle merge conflicts when editing the same files?

## Inspiration

- Clawdbot/Moltbot - persistent proactive agents
- Real monasteries - communal work, governance, rhythms
- IRC - channels as shared context, simple protocol
