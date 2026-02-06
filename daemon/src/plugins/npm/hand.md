Use `npm` for Node.js project operations.

- Prefer `npm ci` when a lockfile exists; otherwise use `npm install`.
- Avoid changing dependency versions unless the task calls for it.
- If the task is about build/test, run the narrowest script first (e.g. `npm test -- <pattern>` when available).
