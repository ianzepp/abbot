use std::path::Path;

use super::{build_environment_layer, build_network_layer};
use super::{SystemBundle, SystemSlot, TarsDials};

use super::trait_catalog;

fn strip_tools_header(md: &str) -> &str {
    let s = md.trim_start();
    if !s.starts_with("## Tools") {
        return md.trim();
    }
    // Drop the first markdown header line plus the following blank line.
    // `describe_tools` renders "## Tools\n\n".
    if let Some(idx) = s.find("\n\n") {
        return s[idx + 2..].trim();
    }
    ""
}

// Fixed slot ordering (0-9) with head as the canonical mapping.
// Other roles use a subset.

pub struct SystemBundler {
    sys: SystemBundle,
}

impl SystemBundler {
    pub fn new() -> Self {
        Self {
            sys: SystemBundle::default(),
        }
    }

    pub fn with_layer(mut self, slot: SystemSlot, content: impl Into<String>) -> Self {
        self.sys.set_slot(slot, content);
        self
    }

    pub fn with_commandments(mut self) -> Self {
        self.sys
            .set_slot(SystemSlot::Commandments, include_str!("commandments.md"));
        self
    }

    pub fn render_tools_section(title: &str, tools_md: &str) -> String {
        let body = strip_tools_header(tools_md);
        if body.trim().is_empty() {
            return String::new();
        }
        format!("## {}\n\n{}", title.trim(), body.trim())
    }

    pub fn with_tools_section(mut self, slot: SystemSlot, title: &str, tools_md: &str) -> Self {
        let rendered = Self::render_tools_section(title, tools_md);
        self.sys.set_slot(slot, rendered);
        self
    }

    pub fn render_external_tools_section(lines_md: &str) -> String {
        let body = lines_md.trim();
        if body.is_empty() {
            return String::new();
        }
        format!("## External Tools (user)\n\n{}", body)
    }

    pub fn with_environment_and_network(mut self, workspace_root: &Path) -> Self {
        let env = build_environment_layer(Some(workspace_root));
        let net = build_network_layer();
        self.sys.set_slot(
            SystemSlot::Environment,
            format!("{}\n\n{}", env.trim(), net.trim()),
        );
        self
    }

    pub fn with_tone(mut self, tars: &TarsDials, traits: &[String]) -> Self {
        let mut parts = Vec::new();
        let t = tars.render();
        if !t.trim().is_empty() {
            parts.push(t);
        }
        let tr = trait_catalog::render_traits(traits);
        if !tr.trim().is_empty() {
            parts.push(tr);
        }
        if !parts.is_empty() {
            self.sys.set_slot(SystemSlot::Tone, parts.join("\n\n"));
        }
        self
    }

    pub fn build(self) -> String {
        self.sys.render()
    }
}
