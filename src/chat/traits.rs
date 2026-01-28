use std::path::Path;

#[derive(Clone, Debug)]
pub struct Trait {
    name: String,
    content: String,
}

impl Trait {
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
        }
    }

    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let content = std::fs::read_to_string(path)?;
        Ok(Self::new(name, content))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn render(&self) -> String {
        format!("<trait name=\"{}\">\n{}\n</trait>", self.name, self.content.trim())
    }
}

pub fn render_all(traits: &[Trait]) -> String {
    traits
        .iter()
        .map(|t| t.render())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn load_dir(path: impl AsRef<Path>, order: &[&str]) -> std::io::Result<Vec<Trait>> {
    let path = path.as_ref();
    let mut traits = Vec::new();

    for name in order {
        let file_path = path.join(format!("{}.md", name));
        if file_path.exists() {
            traits.push(Trait::load(&file_path)?);
        }
    }

    Ok(traits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trait_render() {
        let t = Trait::new("tools", "- !bash - run commands\n- !help - show help");
        assert_eq!(
            t.render(),
            "<trait name=\"tools\">\n- !bash - run commands\n- !help - show help\n</trait>"
        );
    }

    #[test]
    fn test_render_all() {
        let traits = vec![
            Trait::new("system", "You are a bot."),
            Trait::new("tools", "- !help"),
        ];
        let rendered = render_all(&traits);
        assert!(rendered.contains("<trait name=\"system\">"));
        assert!(rendered.contains("<trait name=\"tools\">"));
    }
}
