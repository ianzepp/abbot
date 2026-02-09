-- Unify tasks, needs, wants into a single `entities` table.
--
-- Fixed columns: id, kind, status, priority, room, prompt, created_at, updated_at, data
-- The `data` column holds a JSON blob for kind-specific extras.
-- Callers continue passing table="tasks"/"needs"/"wants" — the EMS service
-- maps that to `kind` internally.

CREATE TABLE IF NOT EXISTS "entities" (
    "id"         TEXT PRIMARY KEY,
    "kind"       TEXT NOT NULL,
    "status"     TEXT NOT NULL DEFAULT 'pending',
    "priority"   INTEGER NOT NULL DEFAULT 2,
    "room"       TEXT NOT NULL DEFAULT 'main',
    "prompt"     TEXT NOT NULL DEFAULT '',
    "created_at" TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    "updated_at" TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    "data"       TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_entities
    ON "entities" ("kind", "status", "priority", "room", "created_at");

-- Migrate tasks: prompt stays as prompt, priority defaults to 2 (normal).
-- Extras (input, notify_scope, lease_owner, leased_at, started_at, completed_at,
-- result, error, head_id, reply_to, batch_calls) go into data blob.
INSERT INTO "entities" ("id", "kind", "status", "priority", "room", "prompt", "created_at", "updated_at", "data")
SELECT
    "id",
    'task',
    "status",
    2,
    "room",
    "prompt",
    "created_at",
    "updated_at",
    json_object(
        'input',        COALESCE("input", ''),
        'notify_scope', "notify_scope",
        'lease_owner',  "lease_owner",
        'leased_at',    "leased_at",
        'started_at',   "started_at",
        'completed_at', "completed_at",
        'result',       "result",
        'error',        "error",
        'head_id',      COALESCE("head_id", 'unknown'),
        'reply_to',     "reply_to",
        'batch_calls',  "batch_calls"
    )
FROM "tasks";

-- Migrate needs: instruction → prompt, priority_rank → priority (INTEGER).
-- Extras (priority label, actor, context, reply_to, reconvene, fulfilled_at,
-- fulfill_result) go into data blob.
INSERT INTO "entities" ("id", "kind", "status", "priority", "room", "prompt", "created_at", "updated_at", "data")
SELECT
    "id",
    'need',
    "status",
    "priority_rank",
    "room",
    "instruction",
    "created_at",
    "updated_at",
    json_object(
        'priority_label', "priority",
        'actor',          COALESCE("actor", 'unknown'),
        'context',        COALESCE("context", ''),
        'reply_to',       "reply_to",
        'reconvene',      "reconvene",
        'fulfilled_at',   "fulfilled_at",
        'fulfill_result', "fulfill_result"
    )
FROM "needs";

-- Migrate wants: want → prompt, text priority → integer.
-- Extras (context, source, promoted_need_id) go into data blob.
INSERT INTO "entities" ("id", "kind", "status", "priority", "room", "prompt", "created_at", "updated_at", "data")
SELECT
    "id",
    'want',
    "status",
    CASE "priority"
        WHEN 'urgent' THEN 0
        WHEN 'high'   THEN 1
        WHEN 'normal' THEN 2
        WHEN 'low'    THEN 3
        ELSE 2
    END,
    'main',
    "want",
    "created_at",
    "updated_at",
    json_object(
        'context',          COALESCE("context", ''),
        'source',           COALESCE("source", 'mind'),
        'promoted_need_id', "promoted_need_id"
    )
FROM "wants";

DROP TABLE IF EXISTS "tasks";
DROP TABLE IF EXISTS "needs";
DROP TABLE IF EXISTS "wants"
