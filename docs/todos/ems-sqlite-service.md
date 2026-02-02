# EMS (SQLite) Service + Tools (Spec)

Goal: add a small, sandbox-local SQLite service (“EMS”) that LLM agents can use as durable structured state via a minimal set of tools.

This is intentionally not the full monk-os-kernel EMS (no meta-model, observers, or cross-DB support). Think “safe sugar” over SQLite.

## Motivation

- Long-lived agents need durable, queryable state that is not just transcripts.
- SQLite per sandbox avoids external DB dependencies.
- Tools should be easy for LLMs to use while preventing SQL/identifier injection.

## Scope

In-scope:

- A new runtime service that owns a sandbox-local SQLite database.
- A small set of head tools for common operations (insert/select/update/delete) plus raw query/exec.
- Identifier validation + parameter binding for safety.
- JSON encoding for nested values.

Out-of-scope (initially):

- Namespaces / schemas.
- Meta-model tables (`models`, `fields`, `tracked`) or automatic “typed schema management”.
- Observer pipelines, triggers, replication.
- Any requirement for Postgres.

## Storage + Lifecycle

- Path: `~/.local/abbot/<sandbox>/ems.sqlite`
- Lifecycle: created on demand when the first EMS tool is invoked.
- SQLite settings:
  - WAL mode
  - `busy_timeout` set to a reasonable default (e.g. 5s)
  - foreign keys enabled (optional; default on if we provide auto-DDL)

## Namespacing

- No namespacing.
- All tables live in the default SQLite namespace.
- Reserve the string `main` only as a conceptual label (not a prefix or schema).

## Service API (Internal)

`EmsService` responsibilities:

- Open/initialize the DB connection for a given sandbox.
- Apply connection pragmas.
- Provide a small API used by tool handlers:
  - `query(sql, params) -> rows`
  - `exec(sql, params) -> changes/count`
  - `insert(table, values, options) -> inserted row (or last_insert_rowid)`
  - `select(table, options) -> rows`
  - `update(table, where, changes) -> count`
  - `delete(table, where) -> count`

Concurrency notes:

- Prefer a single connection per sandbox with a mutex, or a small pool.
- All writes should be serialized (SQLite is single-writer anyway).

## Tool Surface (External)

### 1) `ems_query`

Read-only SQL.

Inputs:

- `sql: string`
- `params?: array<any>`

Behavior:

- Must be parameterized (values through `params`).
- Should reject obviously mutating statements (e.g. `INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`).
- Returns rows as JSON array of objects.

### 2) `ems_exec`

Mutating SQL (DDL/DML).

Inputs:

- `sql: string`
- `params?: array<any>`

Behavior:

- Must be parameterized for values.
- Returns `{ changes: number }` (or SQLite changes count) plus optional metadata.

### 3) `ems_insert`

Primary “LLM-friendly” write API.

Inputs:

- `table: string`
- `values: object` (string -> any)
- `options?: object`
  - `create_table?: boolean` (default: true)
  - `add_columns?: boolean` (default: true)
  - `pk?: string` (default: `id`)
  - `id?: string` (optional override if `pk` is `id`)

Behavior:

- Validate identifiers (table + all keys) as safe SQL identifiers.
- Encode objects/arrays as JSON `TEXT`.
- If `create_table` is true and table does not exist:
  - Create it with at least the pk column.
  - Create additional columns for each key in `values`.
- If `add_columns` is true and keys are missing as columns:
  - `ALTER TABLE ADD COLUMN` for those keys.
- Insert the row using bound parameters.
- Return the inserted row if practical; otherwise return `{ pk: <value> }`.

Type mapping (minimal):

- `string` -> `TEXT`
- `number` -> `REAL` (or `INTEGER` if integral; optional)
- `boolean` -> `INTEGER` (0/1)
- `null/undefined` -> `NULL` (column type defaults to `TEXT` unless inferred)
- `object/array` -> JSON `TEXT`

### 4) `ems_select`

Primary “LLM-friendly” read API.

Inputs:

- `table: string`
- `options?: object`
  - `where?: object` (equality-only in v1)
  - `columns?: string[]` (default: `*`)
  - `order_by?: string` (single column; optional `-col` for desc)
  - `limit?: number`
  - `offset?: number`

Behavior:

- Validate identifiers.
- Build SQL using only `=` predicates with bound params.
- Return rows.

### 5) `ems_update`

Inputs:

- `table: string`
- `where: object`
- `changes: object`

Behavior:

- Validate identifiers.
- Bound params for values.
- Optionally `add_columns` similar to insert (either always on, or gated by an option).
- Return `{ changes: number }`.

### 6) `ems_delete`

Inputs:

- `table: string`
- `where: object`

Behavior:

- Validate identifiers.
- Equality-only where.
- Return `{ changes: number }`.

## Identifier Safety

To avoid SQL injection through identifiers, enforce:

- Table/column pattern: `^[A-Za-z_][A-Za-z0-9_]*$`
- Disallow SQLite internal tables (`sqlite_%`).

All values must be bound parameters; never interpolate user-provided values into SQL.

## Access Control (Abbot)

- `ems_query` is `ReadOnly`.
- All other EMS tools are `Mutating`.

Default exposure:

- Heads: all EMS tools.
- Hands: none initially (consistent with current “hands are read-only” policy). If needed later, consider allowing `ems_select` only.

## Error Model

Return structured tool errors consistent with existing tool errors:

- `E_INVALID_ARGS`: invalid identifiers, empty table, invalid options.
- `E_DB`: SQLite errors (wrapped, message clipped).
- `E_FORBIDDEN`: blocked SQL in `ems_query`.

## Telemetry

- Add tracing spans around each tool call with:
  - tool name
  - table (if applicable)
  - duration
  - rows returned / changes

## First Milestone

- Add `EmsService` that opens `ems.sqlite` per sandbox and applies pragmas.
- Add head tools: `ems_query`, `ems_exec`, `ems_insert`, `ems_select`.
- Implement identifier validation + JSON encoding.
- Keep filtering to equality-only.

## Follow-ups (Optional)

- Add `ems_update` and `ems_delete`.
- Add simple operators in `where` (gt/lt/in/like) with explicit syntax.
- Add an optional `ems_schema(table)` tool for introspection (`PRAGMA table_info`).
- Add a tiny migration/version table (`ems_meta`) if we introduce defaults that require it.
