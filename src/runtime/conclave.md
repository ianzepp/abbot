# Conclave

The Conclave is a periodic deliberation where three managers (MindManager, HeadManager, HandManager) review system activity and reach consensus on what needs and wants to create.

## Purpose

The Conclave exists to:
- Notice patterns that individual components miss
- Surface problems before they become critical
- Identify opportunities for improvement
- Balance strategic vision against operational reality
- Create actionable needs and aspirational wants

## Deliberation Scope

The Conclave deliberates on:

**Needs** (immediate, actionable)
- User requests that weren't fully addressed
- Failures or errors requiring attention
- Blocked work that needs unblocking
- Time-sensitive opportunities

**Wants** (aspirational, deferred)
- Improvements noticed but not urgent
- Ideas worth remembering for later
- Patterns that suggest future work
- Technical debt or cleanup opportunities

## Data to Prep

Before convening, gather:

### Recent Activity
- Messages from the last N hours (user conversations, system events)
- Goals created, completed, failed, timed out
- Needs fulfilled, expired, or still pending

### Operational Health
- Tool execution successes and failures
- Error patterns or repeated failures
- Hand utilization and bottlenecks

### Strategic Context
- Long-term memory (LTM) entries
- Current wants pool (what's been deferred)
- Patterns across conversations

### User Signals
- Unanswered questions or unresolved threads
- Frustration indicators (repeated requests, clarifications)
- Implicit needs not explicitly stated

## Consensus Model

- Three managers deliberate in rounds
- Each manager proposes needs/wants and votes on others' proposals
- Consensus requires 2/3 agreement (2 of 3 managers)
- Maximum 5 rounds before timeout
- On timeout, proposals with 2/3 votes still execute

## Output

The Conclave produces:
- **Needs**: Dispatched to NeedService for immediate handling
- **Wants**: Stored in the wants pool for future consideration
- **LTM updates**: Patterns worth remembering long-term
