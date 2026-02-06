//! LLM:Chaos - Generate chaos traits for behavioral variation testing
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall generates randomized "chaos traits" for injecting behavioral variation
//! into agent execution. It's part of Abbot's chaos engineering framework for testing
//! agent robustness against:
//!
//! - **Emotional states**: Anxiety, confidence, frustration, playfulness
//! - **Cognitive biases**: Confirmation bias, anchoring, availability heuristic
//! - **Communication styles**: Verbosity, terseness, formality, casualness
//! - **Error tendencies**: Typos, incomplete thoughts, tangential reasoning
//!
//! **Integration points:**
//! - `runtime::chaos::roll()` for trait generation with weighted selection
//! - LLM system prompts for injecting traits into agent behavior
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{traits, prompt}` where traits is a map of axis→trait
//!   and prompt is a formatted string ready for injection into LLM system message
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Chaos engineering for agents**: Test robustness by introducing controlled variability
//! - **Reproducibility**: Pinned traits enable deterministic replay of specific behaviors
//! - **Gradual escalation**: Trait levels range from subtle (L1) to extreme (L5)
//! - **Axis independence**: Each axis (emotion, bias, style) can be controlled separately
//! - **Opt-out via exclusion**: Tests can exclude specific axes for targeted testing
//!
//! USE CASES
//! =========
//! 1. **Robustness testing**: Does agent handle tasks correctly when anxious or frustrated?
//! 2. **Edge case discovery**: Find bugs that only manifest with specific trait combinations
//! 3. **Behavioral replay**: Pin traits from failed test run to reproduce issue
//! 4. **Stress testing**: Combine multiple extreme traits (L5) to test worst-case scenarios
//! 5. **UX testing**: Observe how different communication styles affect user experience
//!
//! TRAIT AXES
//! ==========
//! WHY: Traits are organized into axes (dimensions of variation). Each axis has
//! multiple levels (L1-L5) representing increasing intensity.
//!
//! EXAMPLE AXES:
//! - `emotion`: ["calm", "anxious_L1", "anxious_L3", "frustrated_L2", ...]
//! - `bias`: ["neutral", "confirmation_bias_L1", "anchoring_L3", ...]
//! - `style`: ["balanced", "verbose_L2", "terse_L4", ...]
//! - `error_tendency`: ["accurate", "typos_L1", "tangents_L2", ...]
//!
//! SELECTION STRATEGY:
//! - Each axis selects ONE trait via weighted random selection
//! - L1 traits have highest weight (common), L5 traits have lowest weight (rare)
//! - Pinned traits override random selection for that axis
//! - Excluded axes are skipped entirely (no trait selected)
//!
//! PINNING
//! =======
//! WHY: Enable deterministic replay of specific behaviors for debugging failed tests.
//!
//! EXAMPLE:
//! ```json
//! {"pin": {"emotion": "anxious_L3", "style": "verbose_L2"}}
//! ```
//!
//! RESULT: Always selects "anxious_L3" for emotion, "verbose_L2" for style,
//! other axes use random selection.
//!
//! EXCLUSION
//! =========
//! WHY: Allow targeted testing by disabling irrelevant axes.
//!
//! EXAMPLE:
//! ```json
//! {"exclude": ["emotion", "error_tendency"]}
//! ```
//!
//! RESULT: Only cognitive biases and communication styles vary, emotion and
//! errors remain neutral/accurate.
//!
//! PROMPT GENERATION
//! =================
//! WHY: Traits must be translated into natural language instructions for LLMs.
//! The `chaos::roll()` function returns a pre-formatted prompt string.
//!
//! EXAMPLE OUTPUT:
//! ```text
//! You are experiencing moderate anxiety (L3) - second-guess decisions frequently.
//! Communication style: Very verbose (L2) - elaborate on every point with examples.
//! Cognitive bias: Mild confirmation bias (L1) - slightly favor information that
//! confirms initial hypotheses.
//! ```
//!
//! INTEGRATION: Prompt is injected into LLM system message via HandConfig/HeadConfig.
//!
//! CONCURRENCY
//! ===========
//! - Chaos trait generation is stateless and CPU-bound (no I/O)
//! - Safe for concurrent execution across multiple task lanes
//! - Each invocation produces independent random traits (unless pinned)
//!
//! PERFORMANCE
//! ===========
//! - Trait selection is fast (<1ms) - just random number generation and string formatting
//! - No network I/O, no file access, no database queries
//! - Memory usage is minimal (single HashMap + String)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Random vs. Systematic**
//!    - CHOSEN: Weighted random selection per axis
//!    - REJECTED: Systematic enumeration of all trait combinations
//!    - WHY: Combinatorial explosion (5^10 = 9.7M combinations for 10 axes with 5 levels)
//!    - IMPLICATION: May miss rare trait combinations, but practical for testing
//!
//! 2. **LLM Injection vs. Code Simulation**
//!    - CHOSEN: Inject traits via LLM system prompt (text instructions)
//!    - REJECTED: Simulate traits in agent code (e.g., randomly inject typos)
//!    - WHY: LLMs naturally interpret behavioral instructions, simpler implementation
//!    - IMPLICATION: Trait effects depend on LLM instruction-following capability

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for generating chaos traits for agent behavioral variation.
///
/// WHY: Enables chaos engineering for agents - inject controlled behavioral
/// variation to test robustness, discover edge cases, and reproduce bugs.
pub struct LlmChaos;

impl LlmChaos {
    /// Create a new `LlmChaos` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChaos {
    fn name(&self) -> &'static str {
        "llm:chaos"
    }

    /// Generate chaos traits for agent behavioral variation.
    ///
    /// WHY: Enables chaos engineering tests by producing randomized traits
    /// (emotions, biases, communication styles) that can be injected into
    /// agent LLM system prompts.
    ///
    /// USE CASE: Invoked by test harnesses or agent initialization to:
    /// - Test agent robustness against behavioral variations
    /// - Reproduce specific behaviors via pinned traits
    /// - Discover edge cases via random trait combinations
    ///
    /// ARGUMENTS:
    /// - `pin` (optional): Map of axis→trait to override random selection
    ///   Example: `{"emotion": "anxious_L3", "style": "verbose_L2"}`
    /// - `exclude` (optional): Array of axis names to skip
    ///   Example: `["emotion", "error_tendency"]`
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{traits, prompt}`:
    ///   - `traits`: Map of axis→selected_trait (e.g., `{"emotion": "anxious_L3"}`)
    ///   - `prompt`: Formatted string ready for LLM system prompt injection
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // WHY: Parse pinned traits (optional). Pinned traits override random selection
        // for specific axes, enabling deterministic replay of behaviors.
        let pinned: std::collections::HashMap<String, String> = data
            .get("pin")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // WHY: Parse excluded axes (optional). Excluded axes are skipped entirely,
        // enabling targeted testing (e.g., test only cognitive biases, no emotions).
        let exclude: Vec<String> = data
            .get("exclude")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // WHY: Delegate to chaos::roll() for weighted random selection of traits
        // across axes. Returns both selected traits (map) and formatted prompt (string).
        let result = crate::runtime::chaos::roll(&pinned, &exclude);

        // WHY: Return Frame::ok with traits + prompt. Traits enable logging/debugging,
        // prompt is ready for injection into LLM system message.
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "traits": result.selections,
                    "prompt": result.prompt,
                }),
            ))
            .await;

        Ok(())
    }
}
