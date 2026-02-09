---
name: sqlite
description: SQLite database management with the sqlite3 CLI via exec:run
category: data
requires:
  - exec
---

# SQLite CLI

Use `exec:run` with `program: "sqlite3"` to query and manage SQLite databases.

## Calling Convention

The sqlite3 CLI takes the database path as the first argument, followed by a SQL statement or dot-command as a string:

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT * FROM users LIMIT 10;"] }
```

Use `cwd` to resolve relative database paths against a VFS-mounted project:

```json
{ "program": "sqlite3", "args": ["app.db", ".tables"], "cwd": "/projects/backend" }
```

For multi-statement scripts, use `stdin`:

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "BEGIN;\nUPDATE users SET active = 0 WHERE last_login < '2025-01-01';\nSELECT changes();\nCOMMIT;" }
```

## Read-Only vs Mutating

Hand agents may only run **read-only** queries (SELECT, EXPLAIN, `.tables`, `.schema`, `.dump`). Mutating statements (INSERT, UPDATE, DELETE, CREATE, DROP, ALTER) require **head or mind** role.

---

## Output Modes

### Default output (pipe-separated)

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT id, name FROM users LIMIT 5;"] }
```

Output: `1|alice\n2|bob\n3|carol`

### Column mode with headers

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT id, name, email FROM users LIMIT 5;"] }
```

### CSV output

```json
{ "program": "sqlite3", "args": ["-header", "-csv", "data.db", "SELECT * FROM users;"] }
```

### JSON output

```json
{ "program": "sqlite3", "args": ["-json", "data.db", "SELECT * FROM users LIMIT 5;"] }
```

### Tab-separated output

```json
{ "program": "sqlite3", "args": ["-separator", "\t", "-header", "data.db", "SELECT * FROM users;"] }
```

### Line mode (one value per line, good for wide rows)

```json
{ "program": "sqlite3", "args": ["-line", "data.db", "SELECT * FROM users WHERE id = 1;"] }
```

---

## Schema Inspection

### List all tables

```json
{ "program": "sqlite3", "args": ["data.db", ".tables"] }
```

### Show CREATE statement for a table

```json
{ "program": "sqlite3", "args": ["data.db", ".schema users"] }
```

### Show all schemas

```json
{ "program": "sqlite3", "args": ["data.db", ".schema"] }
```

### Show column names and types for a table

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA table_info('users');"] }
```

With readable output:

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "PRAGMA table_info('users');"] }
```

### List indexes on a table

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA index_list('users');"] }
```

### Show index columns

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA index_info('idx_users_email');"] }
```

### List all indexes in the database

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT name, tbl_name FROM sqlite_master WHERE type = 'index' ORDER BY tbl_name;"] }
```

### Show foreign keys for a table

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA foreign_key_list('orders');"] }
```

### Check database integrity

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA integrity_check;"] }
```

---

## Querying Data

### Basic SELECT

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT id, name, email FROM users WHERE active = 1 LIMIT 20;"] }
```

### Count rows

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT COUNT(*) FROM users;"] }
```

### Count by group

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT status, COUNT(*) as cnt FROM orders GROUP BY status ORDER BY cnt DESC;"] }
```

### Aggregate stats

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT MIN(created_at), MAX(created_at), COUNT(*) FROM events;"] }
```

### Search with LIKE

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT * FROM users WHERE name LIKE '%smith%' LIMIT 10;"] }
```

### Join tables

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT u.name, o.id, o.total FROM users u JOIN orders o ON u.id = o.user_id ORDER BY o.total DESC LIMIT 10;"] }
```

### Subquery

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders WHERE total > 100);"] }
```

### EXPLAIN QUERY PLAN

```json
{ "program": "sqlite3", "args": ["data.db", "EXPLAIN QUERY PLAN SELECT * FROM users WHERE email = 'alice@example.com';"] }
```

---

## Working with JSON Data

### Extract a JSON field

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT id, json_extract(data, '$.name') as name FROM entities WHERE kind = 'task';"] }
```

### Filter by JSON field

```json
{ "program": "sqlite3", "args": ["-header", "-column", "data.db", "SELECT * FROM entities WHERE json_extract(data, '$.priority') = 'high';"] }
```

### Update a JSON field

```json
{ "program": "sqlite3", "args": ["data.db", "UPDATE entities SET data = json_set(data, '$.status', 'done') WHERE id = 42;"] }
```

### Insert with JSON data

```json
{ "program": "sqlite3", "args": ["data.db", "INSERT INTO entities (kind, data) VALUES ('task', json('{\"title\": \"Fix bug\", \"priority\": \"high\"}'));"] }
```

### Patch JSON (merge)

```json
{ "program": "sqlite3", "args": ["data.db", "UPDATE entities SET data = json_patch(data, '{\"status\": \"done\"}') WHERE id = 42;"] }
```

### List JSON object keys

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT DISTINCT key FROM entities, json_each(entities.data) WHERE kind = 'task';"] }
```

---

## Modifying Data

### INSERT

```json
{ "program": "sqlite3", "args": ["data.db", "INSERT INTO users (name, email) VALUES ('alice', 'alice@example.com');"] }
```

### INSERT multiple rows

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "INSERT INTO users (name, email) VALUES ('alice', 'alice@example.com');\nINSERT INTO users (name, email) VALUES ('bob', 'bob@example.com');" }
```

### UPDATE

```json
{ "program": "sqlite3", "args": ["data.db", "UPDATE users SET active = 1 WHERE email = 'alice@example.com';"] }
```

### DELETE

```json
{ "program": "sqlite3", "args": ["data.db", "DELETE FROM users WHERE active = 0 AND last_login < '2024-01-01';"] }
```

### Check affected rows after a mutation

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "UPDATE users SET active = 0 WHERE last_login < '2025-01-01';\nSELECT changes();" }
```

### UPSERT (INSERT OR REPLACE)

```json
{ "program": "sqlite3", "args": ["data.db", "INSERT INTO settings (key, value) VALUES ('theme', 'dark') ON CONFLICT(key) DO UPDATE SET value = excluded.value;"] }
```

---

## Schema Changes

### Create a table

```json
{ "program": "sqlite3", "args": ["data.db", "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, type TEXT NOT NULL, payload TEXT, created_at TEXT DEFAULT (datetime('now')));"] }
```

### Add a column

```json
{ "program": "sqlite3", "args": ["data.db", "ALTER TABLE users ADD COLUMN phone TEXT;"] }
```

### Rename a table

```json
{ "program": "sqlite3", "args": ["data.db", "ALTER TABLE users RENAME TO accounts;"] }
```

### Rename a column

```json
{ "program": "sqlite3", "args": ["data.db", "ALTER TABLE users RENAME COLUMN name TO full_name;"] }
```

### Create an index

```json
{ "program": "sqlite3", "args": ["data.db", "CREATE INDEX idx_users_email ON users(email);"] }
```

### Create a unique index

```json
{ "program": "sqlite3", "args": ["data.db", "CREATE UNIQUE INDEX idx_users_email ON users(email);"] }
```

### Drop a table

```json
{ "program": "sqlite3", "args": ["data.db", "DROP TABLE IF EXISTS old_table;"] }
```

### Drop an index

```json
{ "program": "sqlite3", "args": ["data.db", "DROP INDEX IF EXISTS idx_old;"] }
```

---

## Transactions

### Wrap multiple statements in a transaction

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "BEGIN;\nUPDATE accounts SET balance = balance - 100 WHERE id = 1;\nUPDATE accounts SET balance = balance + 100 WHERE id = 2;\nCOMMIT;" }
```

### Check transaction result

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "BEGIN;\nDELETE FROM sessions WHERE expires_at < datetime('now');\nSELECT changes();\nCOMMIT;" }
```

---

## Database Management

### Dump entire database as SQL

```json
{ "program": "sqlite3", "args": ["data.db", ".dump"] }
```

### Dump a specific table

```json
{ "program": "sqlite3", "args": ["data.db", ".dump users"] }
```

### Database file size

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT page_count * page_size as size_bytes FROM pragma_page_count(), pragma_page_size();"] }
```

### Vacuum (reclaim space)

```json
{ "program": "sqlite3", "args": ["data.db", "VACUUM;"] }
```

### WAL checkpoint

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA wal_checkpoint(TRUNCATE);"] }
```

### Show database settings

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "PRAGMA journal_mode;\nPRAGMA foreign_keys;\nPRAGMA wal_autocheckpoint;\nPRAGMA synchronous;" }
```

### Enable WAL mode

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA journal_mode=WAL;"] }
```

### Enable foreign keys

```json
{ "program": "sqlite3", "args": ["data.db", "PRAGMA foreign_keys = ON;"] }
```

---

## SQLite Functions

### Date/time

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT datetime('now');"] }
{ "program": "sqlite3", "args": ["data.db", "SELECT date('now', '-7 days');"] }
{ "program": "sqlite3", "args": ["data.db", "SELECT strftime('%Y-%m', created_at) as month, COUNT(*) FROM events GROUP BY month;"] }
```

### String functions

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT length(name), upper(name), replace(email, '@', ' [at] ') FROM users LIMIT 5;"] }
```

### Coalesce and nulls

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT name, COALESCE(phone, 'N/A') as phone FROM users;"] }
```

### Random sample

```json
{ "program": "sqlite3", "args": ["data.db", "SELECT * FROM users ORDER BY RANDOM() LIMIT 5;"] }
```

---

## Common Workflows

### Explore an unfamiliar database

1. List tables:
   `sqlite3 data.db ".tables"`
2. Inspect a table's schema:
   `sqlite3 data.db ".schema tablename"`
3. Check column types:
   `sqlite3 -header -column data.db "PRAGMA table_info('tablename');"`
4. Sample data:
   `sqlite3 -header -column data.db "SELECT * FROM tablename LIMIT 10;"`
5. Count rows:
   `sqlite3 data.db "SELECT COUNT(*) FROM tablename;"`

### Diagnose a slow query

1. Run EXPLAIN QUERY PLAN:
   `sqlite3 data.db "EXPLAIN QUERY PLAN SELECT ...;"`
2. Look for `SCAN TABLE` (full scan) vs `SEARCH TABLE USING INDEX` (indexed).
3. Add an index if needed:
   `sqlite3 data.db "CREATE INDEX idx_name ON table(column);"`
4. Re-run EXPLAIN to verify index is used.

### Backup a database

1. Dump to SQL:
   `sqlite3 data.db ".dump" > backup.sql`
2. Or use `.backup` command:
   `sqlite3 data.db ".backup backup.db"`

### Migrate data between tables

Use `stdin` for multi-statement operations:

```json
{ "program": "sqlite3", "args": ["data.db"], "stdin": "BEGIN;\nCREATE TABLE users_new (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT UNIQUE);\nINSERT INTO users_new (id, name, email) SELECT id, name, email FROM users;\nDROP TABLE users;\nALTER TABLE users_new RENAME TO users;\nCOMMIT;" }
```

---

## Safety Notes

- **Read-only** (safe for hand agents): `SELECT`, `EXPLAIN`, `PRAGMA table_info`, `PRAGMA index_list`, `.tables`, `.schema`, `.dump` (read-only export), `PRAGMA integrity_check`, `PRAGMA page_count`
- **Mutating** (requires head/mind): `INSERT`, `UPDATE`, `DELETE`, `CREATE`, `ALTER`, `DROP`, `VACUUM`, `PRAGMA journal_mode=`, `PRAGMA foreign_keys=`
- **Destructive** (use with caution): `DROP TABLE`, `DELETE` without WHERE clause, `VACUUM` on large databases (locks the database)
- Always use `WHERE` clauses with `UPDATE` and `DELETE` — check with a `SELECT` first to verify the affected rows.
- Use `SELECT changes()` after mutations to confirm the number of affected rows.
- Wrap multi-statement mutations in `BEGIN`/`COMMIT` transactions for atomicity.
- SQLite locks the entire database on writes — avoid long-running write transactions.
- Use `-json` output mode when the result will be parsed programmatically.
- `sqlite3` is in the default exec allowlist.
