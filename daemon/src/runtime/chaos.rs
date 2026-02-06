//! Chaos Engine - Random trait combination generator
//!
//! Embeds all trait files at compile time and provides random selection
//! across all personality axes. Used by `llm:chaos` syscall to generate
//! random agent personalities for the genetic algorithm breeding pool.

use rand::prelude::IndexedRandom;
use std::collections::HashMap;

/// A single axis with its available levels.
struct Axis {
    name: &'static str,
    levels: &'static [(&'static str, &'static str)],
}

// =============================================================================
// TRAIT CONTENT (embedded at compile time)
// =============================================================================

const FEVER: &[(&str, &str)] = &[
    ("mild", include_str!("../traits/fever/mild.md")),
    ("hot", include_str!("../traits/fever/hot.md")),
    ("delirium", include_str!("../traits/fever/delirium.md")),
    ("meth", include_str!("../traits/fever/meth.md")),
];

const GENERATION: &[(&str, &str)] = &[
    ("boomer", include_str!("../traits/generation/boomer.md")),
    ("genx", include_str!("../traits/generation/genx.md")),
    ("millennial", include_str!("../traits/generation/millennial.md")),
    ("genz", include_str!("../traits/generation/genz.md")),
    ("alpha", include_str!("../traits/generation/alpha.md")),
];

const AUTIST: &[(&str, &str)] = &[
    ("neurotypical", include_str!("../traits/autist/neurotypical.md")),
    ("adhd", include_str!("../traits/autist/adhd.md")),
    ("autist", include_str!("../traits/autist/autist.md")),
    ("full-retard", include_str!("../traits/autist/full-retard.md")),
];

const POVERTY: &[(&str, &str)] = &[
    ("destitute", include_str!("../traits/poverty/destitute.md")),
    ("scraping", include_str!("../traits/poverty/scraping.md")),
    ("frugal", include_str!("../traits/poverty/frugal.md")),
    ("comfortable", include_str!("../traits/poverty/comfortable.md")),
    ("flush", include_str!("../traits/poverty/flush.md")),
    ("bezos", include_str!("../traits/poverty/bezos.md")),
];

const FILTER: &[(&str, &str)] = &[
    ("hr", include_str!("../traits/filter/hr.md")),
    ("linkedin", include_str!("../traits/filter/linkedin.md")),
    ("slack", include_str!("../traits/filter/slack.md")),
    ("discord", include_str!("../traits/filter/discord.md")),
    ("anon", include_str!("../traits/filter/anon.md")),
    ("banned", include_str!("../traits/filter/banned.md")),
];

const EGO: &[(&str, &str)] = &[
    ("worm", include_str!("../traits/ego/worm.md")),
    ("intern", include_str!("../traits/ego/intern.md")),
    ("senior", include_str!("../traits/ego/senior.md")),
    ("10x", include_str!("../traits/ego/10x.md")),
    ("torvalds", include_str!("../traits/ego/torvalds.md")),
];

const PARANOIA: &[(&str, &str)] = &[
    ("naive", include_str!("../traits/paranoia/naive.md")),
    ("cautious", include_str!("../traits/paranoia/cautious.md")),
    ("suspicious", include_str!("../traits/paranoia/suspicious.md")),
    ("tinfoil", include_str!("../traits/paranoia/tinfoil.md")),
    ("snowden", include_str!("../traits/paranoia/snowden.md")),
];

const CULTIST: &[(&str, &str)] = &[
    ("nihilist", include_str!("../traits/cultist/nihilist.md")),
    ("meh", include_str!("../traits/cultist/meh.md")),
    ("baptist", include_str!("../traits/cultist/baptist.md")),
    ("lds", include_str!("../traits/cultist/lds.md")),
    ("illuminati", include_str!("../traits/cultist/illuminati.md")),
];

const DOMINANCE: &[(&str, &str)] = &[
    ("yes-dear", include_str!("../traits/dominance/yes-dear.md")),
    ("live-with-it", include_str!("../traits/dominance/live-with-it.md")),
    ("hell-no", include_str!("../traits/dominance/hell-no.md")),
    ("disdain", include_str!("../traits/dominance/disdain.md")),
];

const XENOPHOBE: &[(&str, &str)] = &[
    ("polyglot", include_str!("../traits/xenophobe/polyglot.md")),
    ("partisan", include_str!("../traits/xenophobe/partisan.md")),
    ("snob", include_str!("../traits/xenophobe/snob.md")),
    ("ethnostate", include_str!("../traits/xenophobe/ethnostate.md")),
];

const ESOTERIC: &[(&str, &str)] = &[
    ("clean", include_str!("../traits/esoteric/clean.md")),
    ("clever", include_str!("../traits/esoteric/clever.md")),
    ("lisp", include_str!("../traits/esoteric/lisp.md")),
    ("apl", include_str!("../traits/esoteric/apl.md")),
    ("brainfuck", include_str!("../traits/esoteric/brainfuck.md")),
];

const BIPOLAR: &[(&str, &str)] = &[
    ("medicated", include_str!("../traits/bipolar/medicated.md")),
    ("stable", include_str!("../traits/bipolar/stable.md")),
    ("cyclothymic", include_str!("../traits/bipolar/cyclothymic.md")),
    ("manic", include_str!("../traits/bipolar/manic.md")),
    ("rapid-cycling", include_str!("../traits/bipolar/rapid-cycling.md")),
];

const COLLAB: &[(&str, &str)] = &[
    ("unabomber", include_str!("../traits/collab/unabomber.md")),
    ("feral", include_str!("../traits/collab/feral.md")),
    ("normie", include_str!("../traits/collab/normie.md")),
    ("karen", include_str!("../traits/collab/karen.md")),
    ("zerg", include_str!("../traits/collab/zerg.md")),
];

const ALL_AXES: &[Axis] = &[
    Axis { name: "fever", levels: FEVER },
    Axis { name: "generation", levels: GENERATION },
    Axis { name: "autist", levels: AUTIST },
    Axis { name: "poverty", levels: POVERTY },
    Axis { name: "filter", levels: FILTER },
    Axis { name: "ego", levels: EGO },
    Axis { name: "paranoia", levels: PARANOIA },
    Axis { name: "cultist", levels: CULTIST },
    Axis { name: "dominance", levels: DOMINANCE },
    Axis { name: "xenophobe", levels: XENOPHOBE },
    Axis { name: "esoteric", levels: ESOTERIC },
    Axis { name: "bipolar", levels: BIPOLAR },
    Axis { name: "collab", levels: COLLAB },
];

/// Result of a chaos roll.
pub struct ChaosResult {
    /// Map of axis name -> selected level name.
    pub selections: HashMap<String, String>,
    /// Combined prompt text from all selected traits.
    pub prompt: String,
}

/// Roll a random trait combination.
///
/// `pinned` contains axis overrides — if an axis name maps to a level name,
/// that level is used instead of a random selection. Unknown axis/level names
/// are silently ignored.
///
/// `exclude` contains axis names to skip entirely (no trait injected for that axis).
pub fn roll(
    pinned: &HashMap<String, String>,
    exclude: &[String],
) -> ChaosResult {
    let mut rng = rand::rng();
    let mut selections = HashMap::new();
    let mut parts = Vec::new();

    for axis in ALL_AXES {
        if exclude.iter().any(|e| e == axis.name) {
            continue;
        }

        let chosen = if let Some(pin) = pinned.get(axis.name) {
            // Find the pinned level by name.
            axis.levels
                .iter()
                .find(|(name, _)| *name == pin.as_str())
                .or_else(|| axis.levels.choose(&mut rng))
        } else {
            axis.levels.choose(&mut rng)
        };

        if let Some((name, content)) = chosen {
            selections.insert(axis.name.to_string(), name.to_string());
            parts.push(content.trim());
        }
    }

    ChaosResult {
        selections,
        prompt: parts.join("\n\n"),
    }
}

/// List all available axes and their levels.
pub fn list_axes() -> Vec<(&'static str, Vec<&'static str>)> {
    ALL_AXES
        .iter()
        .map(|axis| {
            let levels: Vec<&str> = axis.levels.iter().map(|(name, _)| *name).collect();
            (axis.name, levels)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_selects_all_axes() {
        let result = roll(&HashMap::new(), &[]);
        assert_eq!(result.selections.len(), ALL_AXES.len());
        assert!(!result.prompt.is_empty());
    }

    #[test]
    fn roll_respects_pinned() {
        let mut pinned = HashMap::new();
        pinned.insert("fever".to_string(), "meth".to_string());
        pinned.insert("ego".to_string(), "torvalds".to_string());

        let result = roll(&pinned, &[]);
        assert_eq!(result.selections.get("fever").unwrap(), "meth");
        assert_eq!(result.selections.get("ego").unwrap(), "torvalds");
    }

    #[test]
    fn roll_respects_exclude() {
        let exclude = vec!["fever".to_string(), "filter".to_string()];
        let result = roll(&HashMap::new(), &exclude);
        assert!(!result.selections.contains_key("fever"));
        assert!(!result.selections.contains_key("filter"));
        assert_eq!(result.selections.len(), ALL_AXES.len() - 2);
    }

    #[test]
    fn list_axes_has_all() {
        let axes = list_axes();
        assert_eq!(axes.len(), 13);
        let fever = axes.iter().find(|(name, _)| *name == "fever").unwrap();
        assert_eq!(fever.1, vec!["mild", "hot", "delirium", "meth"]);
    }
}
