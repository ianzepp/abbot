# Code Formatting Template

This template demonstrates the formatting and documentation style used in this project.

## File-level Documentation

```rust
//! Module Title - Brief Description
//!
//! SECTION HEADER
//! ==============
//! Detailed explanation of the architecture, design, or key concepts.
//! Focus on WHY this exists and HOW it works at a high level.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Key principle 1: Explanation of rationale
//! - Key principle 2: Explanation of trade-offs
//! - Key principle 3: Why this approach was chosen
//!
//! TRADE-OFFS
//! ==========
//! - Decision point 1: Cost/benefit analysis
//! - Decision point 2: What was sacrificed and why it's acceptable
//! - Decision point 3: Performance/complexity considerations
//!
//! SECURITY MODEL
//! ==============
//! - Security consideration 1: How it's addressed
//! - Security consideration 2: Validation approach
//! - Security consideration 3: Trust boundaries
//!
//! CONCURRENCY / PERFORMANCE / OTHER_RELEVANT_SECTION
//! ===================================================
//! Technical details about specific concerns relevant to this module.
```

## Section Dividers

```rust
// =============================================================================
// SECTION NAME (ALL CAPS)
// =============================================================================
//
// Multi-line explanation of what this section contains and why it exists.
// Focus on the purpose and context, not just listing what's below.
//
// SUBSECTION (if needed)
// ----------------------
// Additional context for a logical grouping within the section.
```

## Type/Struct Documentation

```rust
/// Brief one-line description of the type.
///
/// WHY this exists: Detailed explanation of the rationale behind this type,
/// what problem it solves, or what abstraction it provides.
///
/// INVARIANTS (if applicable):
/// -----------
/// INV-1: Specific guarantee that this type maintains
/// INV-2: Another invariant that callers can rely on
/// INV-3: Constraints that must always be true
pub struct Example {
    /// Brief field description. WHY it's needed or what it represents.
    field: Type,
}
```

## Function Documentation

```rust
/// Brief one-line description of what the function does.
///
/// WHY this exists: Explain the rationale or use case for this function.
/// What problem does it solve? Why is this operation needed?
///
/// SECURITY NOTE: (if applicable) Security considerations, validation
/// approach, or trust boundaries for this operation.
///
/// SAFETY: (if applicable) Requirements or preconditions that must be met.
/// What could go wrong if these aren't satisfied?
pub fn example(&self, param: Type) -> Result<Output, Error> {
    // -------------------------------------------------------------------------
    // PHASE 1: DESCRIPTIVE NAME
    // Brief explanation of what this phase accomplishes and why
    // -------------------------------------------------------------------------
    let step1 = self.prepare(param)?;

    // -------------------------------------------------------------------------
    // PHASE 2: NEXT STEP
    // Explanation of this phase and how it builds on the previous
    // -------------------------------------------------------------------------
    let result = self.execute(step1)?;

    // Single-line operations can have inline comments explaining WHY
    let transformed = result.transform(); // WHY: specific reason this is needed

    Ok(transformed)
}
```

## Helper Functions

```rust
/// Brief description of what this helper does.
///
/// WHY: Explain why this helper exists rather than inline code.
/// What abstraction or reuse does it provide?
///
/// TRADE-OFF: (if applicable) Document any performance, safety, or
/// complexity trade-offs made in this implementation.
fn helper_function(input: &Type) -> Output {
    match input {
        // Branch explanation focuses on WHY this case exists
        Pattern1 => output1,
        // Not just "handle pattern 2" but why pattern 2 needs special handling
        Pattern2 => output2,
    }
}
```

## Error Types

```rust
#[derive(Debug)]
pub struct ModuleError {
    pub code: String,
    pub message: String,
}

impl ModuleError {
    /// Create an error for specific scenario.
    ///
    /// WHY separate constructors: Allows consistent error codes and
    /// structured error handling throughout the module.
    pub fn scenario_name(details: &str) -> Self {
        Self {
            code: "E_SCENARIO".to_string(),
            message: format!("description: {}", details),
        }
    }
}
```

## Complex Operations with Phases

```rust
pub fn complex_operation(&mut self, input: &Input) -> Result<Output, Error> {
    validate_input(input)?;

    // -------------------------------------------------------------------------
    // PHASE 1: DESCRIPTIVE PHASE NAME
    // Detailed explanation of what this phase does and why it's necessary.
    // What invariants does it establish for subsequent phases?
    // -------------------------------------------------------------------------
    let prepared = self.phase_one(input)?;

    // -------------------------------------------------------------------------
    // PHASE 2: NEXT LOGICAL STEP
    // How this builds on phase 1, what problem it solves
    // -------------------------------------------------------------------------
    let mut builder = Builder::new();

    for item in prepared.items() {
        // WHY: specific reason for this operation
        builder.add(transform(item));
    }

    // -------------------------------------------------------------------------
    // PHASE 3: FINALIZATION
    // What this phase accomplishes, why it's separate from phase 2
    // -------------------------------------------------------------------------
    let result = builder.finalize()?;

    Ok(result)
}
```

## Key Principles

1. **WHY over WHAT**: Comments explain rationale, not just description
2. **Trade-offs explicit**: Document what was sacrificed and why it's acceptable
3. **Security noted**: Call out validation, trust boundaries, injection risks
4. **Phases marked**: Complex operations broken into labeled phases with dividers
5. **Invariants documented**: Guarantees that code maintains are explicit
6. **Context provided**: Each section explains its purpose in the larger system
7. **No numbered comments**: Avoid `// 1.`, `// 2.` style; use descriptive phase names
8. **Flat over nested**: Prefer early returns and flat structure over deep nesting
