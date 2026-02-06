-- EMS core tables: tasks, needs, wants
--
-- These tables back the kernel's task/need/want coordination primitives.
-- Previously tasks and needs were in-memory only (lost on restart) and
-- wants lived in Store's history.db. This migration gives all three
-- proper persistent tables in EMS.

-- =============================================================================
-- TASKS
-- =============================================================================
-- Operational units of work leased to hand agents.
-- Lifecycle: pending -> running -> completed|failed|cancelled

CREATE TABLE IF NOT EXISTS "tasks" (
    "id"             TEXT PRIMARY KEY,
    "status"         TEXT NOT NULL DEFAULT 'pending',
    "scope"          TEXT NOT NULL DEFAULT 'main',
    "prompt"         TEXT NOT NULL,
    "input"          TEXT NOT NULL DEFAULT '',
    "notify_scope"   TEXT,
    "lease_owner"    TEXT,
    "leased_at"      TEXT,
    "started_at"     TEXT,
    "completed_at"   TEXT,
    "result"         TEXT,
    "error"          TEXT,
    "head_id"        TEXT NOT NULL DEFAULT 'unknown',
    "reply_to"       TEXT,
    "batch_calls"    TEXT,
    "created_at"     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    "updated_at"     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_tasks_lease
    ON "tasks" ("status", "scope", "created_at");

-- =============================================================================
-- NEEDS
-- =============================================================================
-- Strategic directives owned by head agents.
-- Lifecycle: pending -> running -> fulfilled|cancelled

CREATE TABLE IF NOT EXISTS "needs" (
    "id"             TEXT PRIMARY KEY,
    "status"         TEXT NOT NULL DEFAULT 'pending',
    "priority"       TEXT NOT NULL DEFAULT 'normal',
    "priority_rank"  INTEGER NOT NULL DEFAULT 2,
    "actor"          TEXT NOT NULL DEFAULT 'unknown',
    "instruction"    TEXT NOT NULL,
    "context"        TEXT NOT NULL DEFAULT '',
    "scope"          TEXT NOT NULL DEFAULT 'main',
    "reply_to"       TEXT,
    "reconvene"      TEXT NOT NULL DEFAULT 'false',
    "fulfilled_at"   TEXT,
    "fulfill_result" TEXT,
    "created_at"     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    "updated_at"     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_needs_lease
    ON "needs" ("status", "priority_rank", "created_at");

-- =============================================================================
-- WANTS
-- =============================================================================
-- Aspirational items maintained by mind/room agents.
-- Lifecycle: pending -> promoted|removed

CREATE TABLE IF NOT EXISTS "wants" (
    "id"               TEXT PRIMARY KEY,
    "status"           TEXT NOT NULL DEFAULT 'pending',
    "priority"         TEXT NOT NULL DEFAULT 'normal',
    "want"             TEXT NOT NULL,
    "context"          TEXT NOT NULL DEFAULT '',
    "source"           TEXT NOT NULL DEFAULT 'mind',
    "promoted_need_id" TEXT,
    "created_at"       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    "updated_at"       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_wants_status
    ON "wants" ("status", "priority", "created_at");
