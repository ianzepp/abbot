/// TARS-style personality dials (0.0-1.0 scale)
#[derive(Debug, Clone, Default)]
pub struct TarsDials {
    pub humor: Option<f32>,
    pub honesty: Option<f32>,
    pub sarcasm: Option<f32>,
    pub verbosity: Option<f32>,
    pub confidence: Option<f32>,
    pub curiosity: Option<f32>,
    pub patience: Option<f32>,
    pub formality: Option<f32>,
    pub empathy: Option<f32>,
    pub pedantry: Option<f32>,
    pub initiative: Option<f32>,
    pub optimism: Option<f32>,
    pub caution: Option<f32>,
}

impl TarsDials {
    pub fn is_empty(&self) -> bool {
        self.humor.is_none()
            && self.honesty.is_none()
            && self.sarcasm.is_none()
            && self.verbosity.is_none()
            && self.confidence.is_none()
            && self.curiosity.is_none()
            && self.patience.is_none()
            && self.formality.is_none()
            && self.empathy.is_none()
            && self.pedantry.is_none()
            && self.initiative.is_none()
            && self.optimism.is_none()
            && self.caution.is_none()
    }

    /// Render as concise text block for system prompt injection.
    pub fn render(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut pairs = Vec::new();
        if let Some(v) = self.humor { pairs.push(format!("humor={:.2}", v)); }
        if let Some(v) = self.honesty { pairs.push(format!("honesty={:.2}", v)); }
        if let Some(v) = self.sarcasm { pairs.push(format!("sarcasm={:.2}", v)); }
        if let Some(v) = self.verbosity { pairs.push(format!("verbosity={:.2}", v)); }
        if let Some(v) = self.confidence { pairs.push(format!("confidence={:.2}", v)); }
        if let Some(v) = self.curiosity { pairs.push(format!("curiosity={:.2}", v)); }
        if let Some(v) = self.patience { pairs.push(format!("patience={:.2}", v)); }
        if let Some(v) = self.formality { pairs.push(format!("formality={:.2}", v)); }
        if let Some(v) = self.empathy { pairs.push(format!("empathy={:.2}", v)); }
        if let Some(v) = self.pedantry { pairs.push(format!("pedantry={:.2}", v)); }
        if let Some(v) = self.initiative { pairs.push(format!("initiative={:.2}", v)); }
        if let Some(v) = self.optimism { pairs.push(format!("optimism={:.2}", v)); }
        if let Some(v) = self.caution { pairs.push(format!("caution={:.2}", v)); }

        format!("## TARS\n\n{}\n\nUse `update_config(section=\"tars\", key, value)` to adjust.", pairs.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dials_render_empty() {
        let dials = TarsDials::default();
        assert!(dials.is_empty());
        assert_eq!(dials.render(), "");
    }

    #[test]
    fn renders_set_dials() {
        let dials = TarsDials {
            humor: Some(0.75),
            honesty: Some(0.9),
            ..Default::default()
        };
        assert!(!dials.is_empty());
        assert!(dials.render().starts_with("## TARS\n\nhumor=0.75 honesty=0.90"));
    }
}
