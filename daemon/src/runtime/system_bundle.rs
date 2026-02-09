// Shared system-layer bundler.
//
// This is intentionally simple: fixed 0-9 slots, some roles leave slots empty.
// Higher-level bundlers (head/mind/hand) remain responsible for history and
// role-specific user prompts.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum SystemSlot {
    /// Role identity / invariants.
    Core = 0,
    /// Shared commandments / safety constraints.
    Commandments = 1,
    /// Role-specific context (e.g. wake prompt, routing rules).
    Context = 2,
    /// Primary tools for this role.
    ToolsPrimary = 3,
    /// Secondary tools (e.g. delegation contract).
    ToolsSecondary = 4,
    /// External user-provided tools.
    ToolsExternal = 5,
    /// Role-specific behavioral constraints.
    Behavior = 6,
    /// Environment + network facts.
    Environment = 7,
    /// Long-term memory / durable notes.
    Memory = 8,
    /// Tone modifiers (traits).
    Tone = 9,
}

#[derive(Debug, Clone)]
pub struct SystemBundle {
    layers: [Option<String>; 10],
}

impl Default for SystemBundle {
    fn default() -> Self {
        Self {
            layers: [(); 10].map(|_| None),
        }
    }
}

impl SystemBundle {
    pub fn set_slot(&mut self, slot: SystemSlot, content: impl Into<String>) {
        self.set(slot as usize, content)
    }

    pub fn set(&mut self, idx: usize, content: impl Into<String>) {
        if idx >= 10 {
            return;
        }
        let s = content.into();
        let s = s.trim();
        if s.is_empty() {
            self.layers[idx] = None;
        } else {
            self.layers[idx] = Some(s.to_string());
        }
    }

    pub fn render(&self) -> String {
        self.layers
            .iter()
            .filter_map(|s| s.as_deref())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
