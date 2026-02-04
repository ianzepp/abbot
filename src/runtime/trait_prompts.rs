use super::{AutistMode, FeverMode, FilterMode, GenerationMode, PovertyMode, TarsDials};

fn fever_md(mode: &FeverMode) -> Option<&'static str> {
    match mode {
        FeverMode::None => None,
        FeverMode::Mild => Some(include_str!("../traits/fever/mild.md")),
        FeverMode::Hot => Some(include_str!("../traits/fever/hot.md")),
        FeverMode::Delirium => Some(include_str!("../traits/fever/delirium.md")),
        FeverMode::Meth => Some(include_str!("../traits/fever/meth.md")),
    }
}

fn generation_md(mode: &GenerationMode) -> Option<&'static str> {
    match mode {
        GenerationMode::None => None,
        GenerationMode::Boomer => Some(include_str!("../traits/generation/boomer.md")),
        GenerationMode::GenX => Some(include_str!("../traits/generation/genx.md")),
        GenerationMode::Millennial => Some(include_str!("../traits/generation/millennial.md")),
        GenerationMode::GenZ => Some(include_str!("../traits/generation/genz.md")),
        GenerationMode::Alpha => Some(include_str!("../traits/generation/alpha.md")),
    }
}

fn autist_md(mode: &AutistMode) -> Option<&'static str> {
    match mode {
        AutistMode::None => None,
        AutistMode::Adhd => Some(include_str!("../traits/autist/adhd.md")),
        AutistMode::Neurotypical => Some(include_str!("../traits/autist/neurotypical.md")),
        AutistMode::Autist => Some(include_str!("../traits/autist/autist.md")),
        AutistMode::FullRetard => Some(include_str!("../traits/autist/full-retard.md")),
    }
}

fn filter_md(mode: &FilterMode) -> Option<&'static str> {
    match mode {
        FilterMode::None => None,
        FilterMode::Hr => Some(include_str!("../traits/filter/hr.md")),
        FilterMode::LinkedIn => Some(include_str!("../traits/filter/linkedin.md")),
        FilterMode::Slack => Some(include_str!("../traits/filter/slack.md")),
        FilterMode::Discord => Some(include_str!("../traits/filter/discord.md")),
        FilterMode::Anon => Some(include_str!("../traits/filter/anon.md")),
        FilterMode::Banned => Some(include_str!("../traits/filter/banned.md")),
    }
}

fn poverty_md(mode: &PovertyMode) -> Option<&'static str> {
    match mode {
        PovertyMode::None => None,
        PovertyMode::Destitute => Some(include_str!("../traits/poverty/destitute.md")),
        PovertyMode::Scraping => Some(include_str!("../traits/poverty/scraping.md")),
        PovertyMode::Frugal => Some(include_str!("../traits/poverty/frugal.md")),
        PovertyMode::Comfortable => Some(include_str!("../traits/poverty/comfortable.md")),
        PovertyMode::Flush => Some(include_str!("../traits/poverty/flush.md")),
        PovertyMode::Bezos => Some(include_str!("../traits/poverty/bezos.md")),
    }
}

pub fn render_traits(
    fever: &FeverMode,
    generation: &GenerationMode,
    autist: &AutistMode,
    filter: &FilterMode,
    poverty: &PovertyMode,
) -> String {
    let mut parts: Vec<&'static str> = Vec::new();
    if let Some(s) = fever_md(fever) {
        parts.push(s.trim());
    }
    if let Some(s) = generation_md(generation) {
        parts.push(s.trim());
    }
    if let Some(s) = autist_md(autist) {
        parts.push(s.trim());
    }
    if let Some(s) = filter_md(filter) {
        parts.push(s.trim());
    }
    if let Some(s) = poverty_md(poverty) {
        parts.push(s.trim());
    }
    parts.join("\n\n")
}

pub fn render_tars_and_traits(
    tars: &TarsDials,
    fever: &FeverMode,
    generation: &GenerationMode,
    autist: &AutistMode,
    filter: &FilterMode,
    poverty: &PovertyMode,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let t = tars.render();
    if !t.trim().is_empty() {
        parts.push(t.trim().to_string());
    }
    let traits = render_traits(fever, generation, autist, filter, poverty);
    if !traits.trim().is_empty() {
        parts.push(traits.trim().to_string());
    }
    parts.join("\n\n")
}
