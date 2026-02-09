---
name: typescript-node
description: TypeScript and Node.js development with npm and bun via exec:run
category: development
requires:
  - exec
---

# TypeScript, Node.js, npm & Bun

Use `exec:run` with `program: "npm"`, `program: "npx"`, `program: "node"`, or `program: "bun"` for JavaScript/TypeScript development. Use `program: "tsc"` for direct TypeScript compiler invocations (via npx if not globally installed).

## Calling Convention

```json
{ "program": "npm", "args": ["test"] }
{ "program": "npx", "args": ["tsc", "--noEmit"] }
{ "program": "bun", "args": ["test", "src/utils.test.ts"] }
{ "program": "node", "args": ["scripts/migrate.js"] }
```

Use `cwd` to target a specific VFS-mounted project:

```json
{ "program": "npm", "args": ["run", "build"], "cwd": "/projects/frontend" }
```

## Read-Only vs Mutating

Hand agents may only run **read-only** commands (type-check, test, lint, list). Mutating commands (install, publish, build with output, init) require **head or mind** role.

---

## Package Management: npm

### Install all dependencies

```json
{ "program": "npm", "args": ["install"] }
```

### Install a specific package

```json
{ "program": "npm", "args": ["install", "zod"] }
```

### Install as dev dependency

```json
{ "program": "npm", "args": ["install", "--save-dev", "vitest"] }
```

### Uninstall a package

```json
{ "program": "npm", "args": ["uninstall", "old-package"] }
```

### Update all dependencies

```json
{ "program": "npm", "args": ["update"] }
```

### Update a specific package

```json
{ "program": "npm", "args": ["update", "zod"] }
```

### List installed packages (top-level)

```json
{ "program": "npm", "args": ["list", "--depth=0"] }
```

### Check for outdated packages

```json
{ "program": "npm", "args": ["outdated"] }
```

### View package info

```json
{ "program": "npm", "args": ["view", "zod", "versions", "--json"] }
```

### Audit for vulnerabilities

```json
{ "program": "npm", "args": ["audit"] }
```

### Fix audit issues

```json
{ "program": "npm", "args": ["audit", "fix"] }
```

### Clean install (CI-friendly, respects lockfile)

```json
{ "program": "npm", "args": ["ci"] }
```

---

## Package Management: Bun

### Install all dependencies

```json
{ "program": "bun", "args": ["install"] }
```

### Install a specific package

```json
{ "program": "bun", "args": ["add", "zod"] }
```

### Install as dev dependency

```json
{ "program": "bun", "args": ["add", "--dev", "vitest"] }
```

### Remove a package

```json
{ "program": "bun", "args": ["remove", "old-package"] }
```

---

## TypeScript: Type Checking

### Type-check without emitting (via npx)

```json
{ "program": "npx", "args": ["tsc", "--noEmit"] }
```

### Type-check a specific project (tsconfig)

```json
{ "program": "npx", "args": ["tsc", "--noEmit", "--project", "tsconfig.build.json"] }
```

### Type-check with bun

```json
{ "program": "bun", "args": ["run", "tsc", "--noEmit"] }
```

### Show compiler version

```json
{ "program": "npx", "args": ["tsc", "--version"] }
```

### List effective compiler options

```json
{ "program": "npx", "args": ["tsc", "--showConfig"] }
```

---

## TypeScript: Compilation

### Compile to JavaScript

```json
{ "program": "npx", "args": ["tsc"] }
```

### Compile with a specific config

```json
{ "program": "npx", "args": ["tsc", "--project", "tsconfig.build.json"] }
```

### Watch mode

```json
{ "program": "npx", "args": ["tsc", "--watch", "--noEmit"] }
```

---

## Running Scripts

### Run an npm script

```json
{ "program": "npm", "args": ["run", "build"] }
```

### Run with bun

```json
{ "program": "bun", "args": ["run", "build"] }
```

### List available scripts

```json
{ "program": "npm", "args": ["run"] }
```

### Run a TypeScript file directly with bun

```json
{ "program": "bun", "args": ["run", "src/scripts/seed.ts"] }
```

### Run a JavaScript file with node

```json
{ "program": "node", "args": ["dist/server.js"] }
```

### Run a TypeScript file with node (v22.6+ with type stripping)

```json
{ "program": "node", "args": ["--experimental-strip-types", "src/server.ts"] }
```

### Run with environment variables via node

```json
{ "program": "node", "args": ["--env-file=.env", "dist/server.js"] }
```

---

## Testing

### Run tests via npm script

```json
{ "program": "npm", "args": ["test"] }
```

### Run tests with bun's built-in runner

```json
{ "program": "bun", "args": ["test"] }
```

### Run a specific test file

```json
{ "program": "bun", "args": ["test", "src/utils.test.ts"] }
```

### Run tests matching a pattern

```json
{ "program": "npx", "args": ["vitest", "run", "--reporter=verbose", "utils"] }
```

### Run a specific test by name

```json
{ "program": "npx", "args": ["vitest", "run", "-t", "should parse dates correctly"] }
```

### Run tests with coverage

```json
{ "program": "npx", "args": ["vitest", "run", "--coverage"] }
```

### Run Jest tests

```json
{ "program": "npx", "args": ["jest", "--verbose"] }
```

### Run Jest for a specific file

```json
{ "program": "npx", "args": ["jest", "src/utils.test.ts", "--verbose"] }
```

### Run Jest matching a pattern

```json
{ "program": "npx", "args": ["jest", "-t", "should handle errors"] }
```

### Run Mocha tests

```json
{ "program": "npx", "args": ["mocha", "--recursive", "test/"] }
```

---

## Linting & Formatting

### ESLint: check

```json
{ "program": "npx", "args": ["eslint", "src/"] }
```

### ESLint: fix

```json
{ "program": "npx", "args": ["eslint", "--fix", "src/"] }
```

### ESLint: check a specific file

```json
{ "program": "npx", "args": ["eslint", "src/utils.ts"] }
```

### Prettier: check

```json
{ "program": "npx", "args": ["prettier", "--check", "src/"] }
```

### Prettier: format

```json
{ "program": "npx", "args": ["prettier", "--write", "src/"] }
```

### Prettier: format a specific file

```json
{ "program": "npx", "args": ["prettier", "--write", "src/utils.ts"] }
```

### Biome: check (lint + format)

```json
{ "program": "npx", "args": ["biome", "check", "src/"] }
```

### Biome: fix

```json
{ "program": "npx", "args": ["biome", "check", "--fix", "src/"] }
```

---

## Project Inspection

### Show package.json scripts and metadata

```json
{ "program": "node", "args": ["-e", "const p=require('./package.json');console.log(JSON.stringify({name:p.name,version:p.version,scripts:p.scripts},null,2))"] }
```

### Show node version

```json
{ "program": "node", "args": ["--version"] }
```

### Show npm version

```json
{ "program": "npm", "args": ["--version"] }
```

### Show bun version

```json
{ "program": "bun", "args": ["--version"] }
```

### Evaluate a quick expression

```json
{ "program": "node", "args": ["-e", "console.log(process.versions)"] }
```

---

## Build Tools

### Vite: dev server

```json
{ "program": "npx", "args": ["vite"] }
```

### Vite: production build

```json
{ "program": "npx", "args": ["vite", "build"] }
```

### Vite: preview production build

```json
{ "program": "npx", "args": ["vite", "preview"] }
```

### Next.js: build

```json
{ "program": "npx", "args": ["next", "build"] }
```

### Next.js: dev server

```json
{ "program": "npx", "args": ["next", "dev"] }
```

### esbuild: bundle

```json
{ "program": "npx", "args": ["esbuild", "src/index.ts", "--bundle", "--outfile=dist/index.js", "--platform=node"] }
```

---

## Monorepo / Workspace Operations

### npm workspaces: run a script in all packages

```json
{ "program": "npm", "args": ["run", "build", "--workspaces"] }
```

### npm workspaces: run a script in a specific package

```json
{ "program": "npm", "args": ["run", "test", "--workspace=packages/core"] }
```

### npm workspaces: install a dep in a specific package

```json
{ "program": "npm", "args": ["install", "zod", "--workspace=packages/api"] }
```

### bun workspaces: run in a specific package

```json
{ "program": "bun", "args": ["run", "--filter", "packages/core", "test"] }
```

### List workspace packages

```json
{ "program": "npm", "args": ["query", ".workspace"] }
```

---

## Common Workflows

### Pre-commit check (type-check + lint + test)

1. Type-check:
   `npx tsc --noEmit`
2. Lint:
   `npx eslint src/`
3. Test:
   `npm test`

### Add a dependency and verify

1. Install:
   `npm install zod`
2. Type-check:
   `npx tsc --noEmit`
3. Test:
   `npm test`

### Diagnose a type error

1. Run type-check:
   `npx tsc --noEmit`
2. Read the error — look for file path, line number, and error code (e.g., TS2345).
3. If the error is in a dependency, check types are installed:
   `npm list @types/problematic-package`

### Diagnose a test failure

1. Run the failing test with verbose output:
   `npx vitest run --reporter=verbose failing-test-file`
2. Or with Jest:
   `npx jest failing-test-file --verbose`

### Upgrade a major dependency

1. Check current version:
   `npm list target-package`
2. Check available versions:
   `npm view target-package versions --json`
3. Install specific version:
   `npm install target-package@^5.0.0`
4. Type-check:
   `npx tsc --noEmit`
5. Test:
   `npm test`

---

## Safety Notes

- **Read-only** (safe for hand agents): `tsc --noEmit`, `npm test`, `npm run` (listing), `npm list`, `npm outdated`, `npm audit`, `npm view`, `eslint` (without `--fix`), `prettier --check`, `node --version`, `bun test`
- **Mutating** (requires head/mind): `npm install`, `npm uninstall`, `npm update`, `npm audit fix`, `npm ci`, `bun add`, `bun remove`, `tsc` (emitting), `eslint --fix`, `prettier --write`, `npm publish`, build commands that write to `dist/`
- `npm test` and `bun test` execute project code — treat as read-only for access control but be aware they run arbitrary test code.
- Use `npx` to run locally-installed binaries (tsc, eslint, vitest, etc.) without global installs.
- Use `npm ci` instead of `npm install` in CI — it's faster and strictly follows the lockfile.
- Arguments after `--` in npm scripts pass through to the underlying command: `npm test -- --coverage`.
