# Code Formatting Instructions

Apply the formatting and documentation style demonstrated in `skills/formatter.example.md` to code in this project.

## Objective

Transform code to follow the project's documentation standards: emphasizing WHY over WHAT, making trade-offs explicit, documenting security considerations, and organizing complex operations into clearly marked phases.

## When to Apply

- New files being created
- Existing files being significantly refactored
- Functions being rewritten or substantially modified
- When requested explicitly by the user

**Do not** automatically reformat files just because you're reading them or making small edits.

## File-Level Documentation

Every Rust module file should start with:

```rust
//! Module Title - One-line description
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! High-level explanation of the module's purpose, structure, and how it fits
//! into the larger system. Focus on design decisions and system interactions.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Principle 1: WHY this principle guides the design
//! - Principle 2: What problem it solves
//! - Principle 3: What trade-offs it implies
```

Additional sections as relevant:
- `TRADE-OFFS`: Document decisions, what was sacrificed, why it's acceptable
- `SECURITY MODEL`: Trust boundaries, validation approach, injection prevention
- `CONCURRENCY`: Threading model, synchronization strategy
- `PERFORMANCE`: Known bottlenecks, optimization decisions
- `ERROR HANDLING`: Error propagation strategy, recovery approach

## Section Organization

Organize code into logical sections with dividers:

```rust
// =============================================================================
// SECTION NAME (all caps, describes logical grouping)
// =============================================================================
//
// Multi-line comment explaining:
// - What this section contains
// - WHY it exists as a separate section
// - How it relates to other sections
// - Any important context callers should know
```

Common section names:
- `ERRORS` - Error types and constructors
- `TYPES` - Type definitions and data structures
- `SERVICE` or `CORE` - Main implementation
- `HELPERS` - Utility functions
- `VALIDATION` - Input validation functions
- `CONVERSION` - Type conversion utilities

## Type Documentation

```rust
/// Brief description (one line).
///
/// WHY this type exists: Explain the abstraction, what problem it solves,
/// or what invariants it maintains. Don't just restate the type name.
///
/// INVARIANTS: (if applicable)
/// ----------
/// INV-1: Specific guarantee this type maintains
/// INV-2: Constraints that must always hold
///
/// TRADE-OFF: (if applicable) Document design trade-offs made
pub struct TypeName {
    /// Field purpose. WHY it's structured this way.
    field: Type,
}
```

## Function Documentation

For public functions:

```rust
/// Brief description of what this function does (active voice, imperative).
///
/// WHY this exists: The use case, problem solved, or rationale for this
/// operation. What gap in the API does it fill?
///
/// SECURITY NOTE: (if applicable) Validation performed, trust assumptions,
/// injection risks, or why certain operations are/aren't allowed.
///
/// SAFETY: (if applicable) Preconditions required, what could go wrong,
/// requirements for correct usage.
pub fn operation(&self, param: Type) -> Result<Output, Error> {
```

For private/helper functions:

```rust
/// Brief description.
///
/// WHY: Explain why this helper exists rather than inline code. What
/// abstraction does it provide? What duplication does it prevent?
fn helper(input: &Type) -> Output {
```

## Complex Operations: Phase Markers

For functions with multiple logical steps, use phase markers:

```rust
pub fn complex_operation(&mut self, input: &Input) -> Result<Output, Error> {
    // -------------------------------------------------------------------------
    // PHASE 1: DESCRIPTIVE NAME (not "Phase 1", but "SCHEMA EVOLUTION" or "VALIDATION")
    // Explain what this phase accomplishes, why it's necessary, and what
    // invariants it establishes for subsequent phases
    // -------------------------------------------------------------------------
    let result1 = self.step_one(input)?;

    // -------------------------------------------------------------------------
    // PHASE 2: NEXT LOGICAL STEP
    // How this builds on previous phase, what problem it solves
    // -------------------------------------------------------------------------
    let result2 = self.step_two(result1)?;

    // -------------------------------------------------------------------------
    // PHASE 3: FINALIZATION
    // What this accomplishes, why it's separate
    // -------------------------------------------------------------------------
    let final_result = self.finalize(result2)?;

    Ok(final_result)
}
```

**Guidelines for phases:**
- Use descriptive names that indicate WHAT the phase does: "SCHEMA EVOLUTION", "BUILD INSERT STATEMENT", "EXECUTE AND RETURN"
- Include WHY comment explaining the phase's purpose
- Use dashes (`-`) not equals (`=`) for phase dividers
- Only mark phases when there are genuinely distinct logical steps (3+)

## Inline Comments

Comments should explain WHY, not WHAT:

```rust
// GOOD: Explains rationale
let result = transform(input); // WHY: normalization required for SQLite storage

// BAD: Restates the code
let result = transform(input); // Transform the input
```

Special markers:
- `WHY:` - Explain rationale or design decision
- `TRADE-OFF:` - Document what was sacrificed
- `NOTE:` - Important context or gotcha
- `SAFETY:` - Safety requirement or precondition
- `SECURITY:` - Security consideration

## Error Handling

Document error constructors:

```rust
impl ModuleError {
    /// Create error for [scenario].
    ///
    /// WHY separate constructor: Ensures consistent error codes and enables
    /// structured error handling throughout the module.
    pub fn scenario_name(context: impl Into<String>) -> Self {
        Self {
            code: "E_SCENARIO".to_string(),
            message: context.into(),
        }
    }
}
```

## Match Expressions and Branches

```rust
match value {
    // WHY this pattern exists and why it needs special handling
    Pattern1 => handle_case1(),

    // Explain the scenario this covers, not just "handle pattern 2"
    Pattern2 => handle_case2(),
}
```

## What NOT to Do

❌ **Numbered comments**: Avoid `// 1. First step`, `// 2. Second step`
✅ **Use**: Phase markers with descriptive names

❌ **Restating code**: `// Create a new vector`
✅ **Explain WHY**: `// WHY: pre-allocate to avoid reallocation during iteration`

❌ **Vague comments**: `// Handle the input`
✅ **Be specific**: `// WHY: validation prevents SQL injection via identifiers`

❌ **Over-documenting obvious code**: Every line doesn't need a comment
✅ **Document decisions**: Why this approach over alternatives

❌ **What-focused headers**: `// Functions for processing`
✅ **Why-focused headers**: Explain the role in the system architecture

## Application Checklist

When formatting a file, ensure:

- [ ] File-level `//!` documentation with architecture overview
- [ ] Major sections divided with `// ===...===` dividers
- [ ] Section headers have explanatory comments
- [ ] Public types have WHY documentation
- [ ] Public functions explain their purpose/rationale
- [ ] Complex operations use phase markers
- [ ] Trade-offs are documented where decisions were made
- [ ] Security considerations are called out
- [ ] Invariants are explicitly stated
- [ ] Comments focus on WHY not WHAT
- [ ] Error types have documented constructors

## Integration with Development

This formatting style should be:
- Applied to new files as they're created
- Used when substantially refactoring existing code
- Followed in code reviews
- Not applied automatically without user request
- Balanced against the scope of changes (don't reformat entire files for minor edits)

The goal is clarity and maintainability through explicit documentation of decisions, trade-offs, and rationale.
