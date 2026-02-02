# EMS (SQLite) Service + Tools (Spec)

Goal: add a small, sandbox-local SQLite service ("EMS") that LLM agents can use as durable structured state via a minimal set of tools.

This is intentionally not the full monk-os-kernel EMS (no meta-model, observers, or cross-DB support). Think "safe sugar" over SQLite.

## Motivation

- Long-lived agents need durable, queryable state that is not just transcripts.
- SQLite per sandbox avoids external DB dependencies.
- Tools should be easy for LLMs to use while preventing SQL/identifier injection.

## Scope

In-scope:

- A new runtime service that owns a sandbox-local SQLite database.
- A small set of head tools for structured operations (insert/select/update/delete) plus read-only query.
- Identifier validation + parameter binding for safety.
- JSON encoding for nested values.

Out-of-scope (initially):

- Namespaces / schemas.
- Meta-model tables (`models`, `fields`, `tracked`) or automatic "typed schema management".
- Observer pipelines, triggers, replication.
- Any requirement for Postgres.
- Raw mutating SQL (no `ems_exec`; agents use structured tools instead).

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
  - `insert(table, values, options) -> inserted row`
  - `select(table, options) -> rows`
  - `update(table, where, changes) -> count`
  - `delete(table, where) -> count`
  - `describe(table?) -> tables list or column info`

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
- Returns rows as JSON array of objects (no auto-deserialization; raw query has no schema context).

### 2) `ems_insert`

Primary "LLM-friendly" write API.

Inputs:

- `table: string`
- `values: object` (string -> any)
- `options?: object`
  - `create_table?: boolean` (default: true)
  - `add_columns?: boolean` (default: true)

Behavior:

- Validate identifiers (table + all keys) as safe SQL identifiers.
- Encode objects/arrays as JSON `TEXT`.
- If `create_table` is true and table does not exist:
  - Create it with `id TEXT PRIMARY KEY` plus columns for each key in `values`.
- If `add_columns` is true and keys are missing as columns:
  - `ALTER TABLE ADD COLUMN` for those keys.
- Generate a UUID for `id` if not provided in `values`.
- Insert the row using bound parameters with `RETURNING *`.
- Return the full inserted row (with JSON columns deserialized).

Primary key:

- All tables use `id TEXT PRIMARY KEY`.
- IDs are UUIDs, auto-generated on insert if not provided.

Type mapping (minimal):

- `string` -> `TEXT`
- `number` -> `REAL` (or `INTEGER` if integral; optional)
- `boolean` -> `INTEGER` (0/1)
- `null/undefined` -> `NULL` (column type defaults to `TEXT` unless inferred)
- `object/array` -> JSON `TEXT` (auto-deserialized on read if string starts with `{` or `[`)

### 3) `ems_select`

Primary "LLM-friendly" read API.

Inputs:

- `table: string`
- `options?: object`
  - `where?: object` (see Where Syntax below)
  - `columns?: string[]` (default: `*`)
  - `order_by?: string | string[]` (SQL-style: `"col"` or `"col DESC"`)
  - `limit?: number`
  - `offset?: number`

Behavior:

- Validate identifiers.
- Build SQL with bound params.
- Return rows (with JSON columns deserialized to objects/arrays).

### 4) `ems_update`

Inputs:

- `table: string`
- `where: object` (see Where Syntax below)
- `changes: object`
- `options?: object`
  - `add_columns?: boolean` (default: true)

Behavior:

- Validate identifiers.
- Bound params for values.
- If `add_columns` is true and keys in `changes` are missing as columns:
  - `ALTER TABLE ADD COLUMN` for those keys.
- Return `{ changes: number }`.

### 5) `ems_delete`

Inputs:

- `table: string`
- `ids: string[]` (array of row IDs to delete)

Behavior:

- Validate table identifier.
- Delete rows where `id IN (...)` using bound params.
- Return `{ changes: number }`.

### 6) `ems_describe`

Introspection tool.

Inputs:

- `table?: string`

Behavior:

- If no table provided: return list of all user tables (excludes `sqlite_%`).
- If table provided: return column info via `PRAGMA table_info(table)`.

## Where Syntax

The `where` object supports equality and comparison operators:

```typescript
where: {
  status: "active",              // equality (implicit)
  count: { $gt: 10 },            // >
  count: { $gte: 10 },           // >=
  count: { $lt: 100 },           // <
  count: { $lte: 100 },          // <=
  id: { $in: [1, 2, 3] },        // IN (...)
}
```

Supported operators:

- Bare value: equality (`=`)
- Bare `null`: `IS NULL`
- `$gt`: greater than
- `$lt`: less than
- `$gte`: greater than or equal
- `$lte`: less than or equal
- `$in`: set membership

All values are bound parameters. Multiple conditions are ANDed together.

## Order By Syntax

The `order_by` field accepts SQL-style syntax:

```typescript
order_by: "created_at"                    // single column, ascending
order_by: "created_at DESC"               // single column, descending
order_by: ["status", "created_at DESC"]   // multiple columns
```

Column names are validated as safe identifiers. Direction must be `ASC` or `DESC` (case-insensitive); defaults to `ASC` if omitted.

## Identifier Safety

To avoid SQL injection through identifiers, enforce:

- Table/column pattern: `^[A-Za-z_][A-Za-z0-9_]*$`
- Disallow SQLite internal tables (`sqlite_%`).

All values must be bound parameters; never interpolate user-provided values into SQL.

## Access Control (Abbot)

- `ems_query` and `ems_describe` are `ReadOnly`.
- All other EMS tools are `Mutating`.

Default exposure:

- Heads: all EMS tools.
- Hands: none initially (consistent with current "hands are read-only" policy). If needed later, consider allowing `ems_select` and `ems_describe` only.

## Error Model

Return structured tool errors consistent with existing tool errors:

- `E_INVALID_ARGS`: invalid identifiers, empty table, invalid options, unknown operator.
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
- Add head tools: `ems_query`, `ems_insert`, `ems_select`, `ems_update`, `ems_delete`, `ems_describe`.
- Implement identifier validation.
- Implement JSON encoding on write, auto-deserialization on read.
- Implement UUID generation for primary keys.
- Implement where syntax with equality, null, and comparison operators (`$gt`, `$lt`, `$gte`, `$lte`, `$in`).
- Implement order_by with SQL-style syntax.
- Use `RETURNING *` for inserts.

## Follow-ups (Optional)

- Add `$like` operator for pattern matching.
- Add `$ne` (not equal) and `$nin` (not in) operators.
- Add `$or` for disjunctive queries.
- Add a tiny migration/version table (`ems_meta`) if we introduce defaults that require it.
- Add transaction support (`ems_transaction([ops])`).
