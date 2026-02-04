use super::{AutistMode, FeverMode, GenerationMode, TactMode, TarsDials};

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

fn tact_md(mode: &TactMode) -> Option<&'static str> {
    match mode {
        TactMode::None => None,
        TactMode::Sycophant => Some(include_str!("../traits/tact/sycophant.md")),
        TactMode::Therapist => Some(include_str!("../traits/tact/therapist.md")),
        TactMode::Sergeant => Some(include_str!("../traits/tact/sergeant.md")),
        TactMode::GordonRamsay => Some(include_str!("../traits/tact/gordon-ramsay.md")),
        TactMode::Roast => Some(include_str!("../traits/tact/roast.md")),
        TactMode::Marvin => Some(include_str!("../traits/tact/marvin.md")),
    }
}

pub fn render_traits(
    fever: &FeverMode,
    generation: &GenerationMode,
    autist: &AutistMode,
    tact: &TactMode,
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
    if let Some(s) = tact_md(tact) {
        parts.push(s.trim());
    }
    parts.join("\n\n")
}

pub fn render_tars_and_traits(
    tars: &TarsDials,
    fever: &FeverMode,
    generation: &GenerationMode,
    autist: &AutistMode,
    tact: &TactMode,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let t = tars.render();
    if !t.trim().is_empty() {
        parts.push(t.trim().to_string());
    }
    let traits = render_traits(fever, generation, autist, tact);
    if !traits.trim().is_empty() {
        parts.push(traits.trim().to_string());
    }
    parts.join("\n\n")
}
