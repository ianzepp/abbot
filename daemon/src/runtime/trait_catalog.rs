/// Returns the full trait catalog: category names paired with their variant lists.
///
/// Used by the CLI trait picker and for validation.
pub fn trait_categories() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("fever", &["mild", "hot", "delirium", "meth"]),
        (
            "generation",
            &["boomer", "genx", "millennial", "genz", "alpha"],
        ),
        ("autist", &["adhd", "neurotypical", "autist", "full-retard"]),
        (
            "filter",
            &["hr", "linkedin", "slack", "discord", "anon", "banned"],
        ),
        (
            "poverty",
            &[
                "destitute",
                "scraping",
                "frugal",
                "comfortable",
                "flush",
                "bezos",
            ],
        ),
        ("ego", &["worm", "intern", "senior", "10x", "torvalds"]),
        (
            "paranoia",
            &["naive", "cautious", "suspicious", "tinfoil", "snowden"],
        ),
        (
            "cultist",
            &["nihilist", "meh", "baptist", "lds", "illuminati"],
        ),
        (
            "dominance",
            &["yes-dear", "live-with-it", "hell-no", "disdain"],
        ),
        (
            "bipolar",
            &[
                "medicated",
                "stable",
                "cyclothymic",
                "manic",
                "rapid-cycling",
            ],
        ),
        ("xenophobe", &["polyglot", "partisan", "snob", "ethnostate"]),
        ("esoteric", &["clean", "clever", "lisp", "apl", "brainfuck"]),
        ("collab", &["unabomber", "feral", "normie", "karen", "zerg"]),
    ]
}

/// Trait catalog: maps `"category/variant"` strings to embedded `.md` content.
///
/// Add a new trait by dropping a `.md` file into `traits/<category>/<variant>.md`
/// and adding one match arm here.
pub fn load_trait(name: &str) -> Option<&'static str> {
    match name {
        // fever
        "fever/mild" => Some(include_str!("../prompts/traits/fever/mild.md")),
        "fever/hot" => Some(include_str!("../prompts/traits/fever/hot.md")),
        "fever/delirium" => Some(include_str!("../prompts/traits/fever/delirium.md")),
        "fever/meth" => Some(include_str!("../prompts/traits/fever/meth.md")),

        // generation
        "generation/boomer" => Some(include_str!("../prompts/traits/generation/boomer.md")),
        "generation/genx" => Some(include_str!("../prompts/traits/generation/genx.md")),
        "generation/millennial" => Some(include_str!("../prompts/traits/generation/millennial.md")),
        "generation/genz" => Some(include_str!("../prompts/traits/generation/genz.md")),
        "generation/alpha" => Some(include_str!("../prompts/traits/generation/alpha.md")),

        // autist
        "autist/adhd" => Some(include_str!("../prompts/traits/autist/adhd.md")),
        "autist/neurotypical" => Some(include_str!("../prompts/traits/autist/neurotypical.md")),
        "autist/autist" => Some(include_str!("../prompts/traits/autist/autist.md")),
        "autist/full-retard" => Some(include_str!("../prompts/traits/autist/full-retard.md")),

        // filter
        "filter/hr" => Some(include_str!("../prompts/traits/filter/hr.md")),
        "filter/linkedin" => Some(include_str!("../prompts/traits/filter/linkedin.md")),
        "filter/slack" => Some(include_str!("../prompts/traits/filter/slack.md")),
        "filter/discord" => Some(include_str!("../prompts/traits/filter/discord.md")),
        "filter/anon" => Some(include_str!("../prompts/traits/filter/anon.md")),
        "filter/banned" => Some(include_str!("../prompts/traits/filter/banned.md")),

        // poverty
        "poverty/destitute" => Some(include_str!("../prompts/traits/poverty/destitute.md")),
        "poverty/scraping" => Some(include_str!("../prompts/traits/poverty/scraping.md")),
        "poverty/frugal" => Some(include_str!("../prompts/traits/poverty/frugal.md")),
        "poverty/comfortable" => Some(include_str!("../prompts/traits/poverty/comfortable.md")),
        "poverty/flush" => Some(include_str!("../prompts/traits/poverty/flush.md")),
        "poverty/bezos" => Some(include_str!("../prompts/traits/poverty/bezos.md")),

        // ego
        "ego/worm" => Some(include_str!("../prompts/traits/ego/worm.md")),
        "ego/intern" => Some(include_str!("../prompts/traits/ego/intern.md")),
        "ego/senior" => Some(include_str!("../prompts/traits/ego/senior.md")),
        "ego/10x" => Some(include_str!("../prompts/traits/ego/10x.md")),
        "ego/torvalds" => Some(include_str!("../prompts/traits/ego/torvalds.md")),

        // paranoia
        "paranoia/naive" => Some(include_str!("../prompts/traits/paranoia/naive.md")),
        "paranoia/cautious" => Some(include_str!("../prompts/traits/paranoia/cautious.md")),
        "paranoia/suspicious" => Some(include_str!("../prompts/traits/paranoia/suspicious.md")),
        "paranoia/tinfoil" => Some(include_str!("../prompts/traits/paranoia/tinfoil.md")),
        "paranoia/snowden" => Some(include_str!("../prompts/traits/paranoia/snowden.md")),

        // cultist
        "cultist/nihilist" => Some(include_str!("../prompts/traits/cultist/nihilist.md")),
        "cultist/meh" => Some(include_str!("../prompts/traits/cultist/meh.md")),
        "cultist/baptist" => Some(include_str!("../prompts/traits/cultist/baptist.md")),
        "cultist/lds" => Some(include_str!("../prompts/traits/cultist/lds.md")),
        "cultist/illuminati" => Some(include_str!("../prompts/traits/cultist/illuminati.md")),

        // dominance
        "dominance/yes-dear" => Some(include_str!("../prompts/traits/dominance/yes-dear.md")),
        "dominance/live-with-it" => {
            Some(include_str!("../prompts/traits/dominance/live-with-it.md"))
        }
        "dominance/hell-no" => Some(include_str!("../prompts/traits/dominance/hell-no.md")),
        "dominance/disdain" => Some(include_str!("../prompts/traits/dominance/disdain.md")),

        // bipolar
        "bipolar/medicated" => Some(include_str!("../prompts/traits/bipolar/medicated.md")),
        "bipolar/stable" => Some(include_str!("../prompts/traits/bipolar/stable.md")),
        "bipolar/cyclothymic" => Some(include_str!("../prompts/traits/bipolar/cyclothymic.md")),
        "bipolar/manic" => Some(include_str!("../prompts/traits/bipolar/manic.md")),
        "bipolar/rapid-cycling" => Some(include_str!("../prompts/traits/bipolar/rapid-cycling.md")),

        // xenophobe
        "xenophobe/polyglot" => Some(include_str!("../prompts/traits/xenophobe/polyglot.md")),
        "xenophobe/partisan" => Some(include_str!("../prompts/traits/xenophobe/partisan.md")),
        "xenophobe/snob" => Some(include_str!("../prompts/traits/xenophobe/snob.md")),
        "xenophobe/ethnostate" => Some(include_str!("../prompts/traits/xenophobe/ethnostate.md")),

        // esoteric
        "esoteric/clean" => Some(include_str!("../prompts/traits/esoteric/clean.md")),
        "esoteric/clever" => Some(include_str!("../prompts/traits/esoteric/clever.md")),
        "esoteric/lisp" => Some(include_str!("../prompts/traits/esoteric/lisp.md")),
        "esoteric/apl" => Some(include_str!("../prompts/traits/esoteric/apl.md")),
        "esoteric/brainfuck" => Some(include_str!("../prompts/traits/esoteric/brainfuck.md")),

        // collab
        "collab/unabomber" => Some(include_str!("../prompts/traits/collab/unabomber.md")),
        "collab/feral" => Some(include_str!("../prompts/traits/collab/feral.md")),
        "collab/normie" => Some(include_str!("../prompts/traits/collab/normie.md")),
        "collab/karen" => Some(include_str!("../prompts/traits/collab/karen.md")),
        "collab/zerg" => Some(include_str!("../prompts/traits/collab/zerg.md")),

        _ => None,
    }
}

/// Render a list of trait names into a combined markdown string.
pub fn render_traits(names: &[String]) -> String {
    names
        .iter()
        .filter_map(|n| load_trait(n))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_known_trait() {
        assert!(load_trait("fever/mild").is_some());
        assert!(load_trait("collab/zerg").is_some());
    }

    #[test]
    fn returns_none_for_unknown() {
        assert!(load_trait("nonexistent/trait").is_none());
    }

    #[test]
    fn render_empty_list() {
        assert_eq!(render_traits(&[]), "");
    }

    #[test]
    fn render_skips_unknown() {
        let names = vec!["fever/mild".into(), "bogus/nope".into()];
        let rendered = render_traits(&names);
        assert!(!rendered.is_empty());
        assert!(!rendered.contains("bogus"));
    }
}
