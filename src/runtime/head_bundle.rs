use std::sync::Arc;

use crate::bus::{MessageData, MessageOp, Scope, TaskMsg};
use crate::history::Store;

pub struct HeadBundleConfig {
    pub head_id: String,
    pub include_scopes: Vec<Scope>,
    pub max_messages_per_scope: usize,
    pub include_trace: bool,
    pub max_trace_steps: usize,
}

impl Default for HeadBundleConfig {
    fn default() -> Self {
        Self {
            head_id: "Monk".to_string(),
            include_scopes: vec![Scope::from("#general")],
            max_messages_per_scope: 80,
            include_trace: false,
            max_trace_steps: 30,
        }
    }
}

pub struct HeadBundleBuilder {
    store: Arc<Store>,
}

impl HeadBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn build(&self, cfg: &HeadBundleConfig) -> String {
        let grammar = include_str!("../grammar/head.md");
        let system = ""; // placeholder (to be defined later)
        let ltm = self.store.get_head_ltm(&cfg.head_id).unwrap_or_default();
        let stm = self.store.get_head_stm(&cfg.head_id).unwrap_or_default();

        let focus = format!(
            "head_id={}\nscopes={}",
            cfg.head_id,
            cfg.include_scopes
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let mut out = String::new();
        push_block(&mut out, "grammar", grammar.trim());
        push_block(&mut out, "system", system.trim());
        push_block(&mut out, "ltm", ltm.trim());
        push_block(&mut out, "stm", stm.trim());
        push_block(&mut out, "focus", focus.trim());

        // scope/messages
        out.push_str("\n<scope>\n");
        for scope in &cfg.include_scopes {
            let scope_str = scope.to_string();
            let messages = self
                .store
                .recent(&scope_str, cfg.max_messages_per_scope)
                .unwrap_or_default();

            out.push_str(&format!("  <stream scope=\"{}\">\n", escape_attr(&scope_str)));

            for m in messages {
                out.push_str("    ");
                out.push_str(&render_message(&m));
                out.push('\n');
            }

            out.push_str("  </stream>\n");
        }
        out.push_str("</scope>\n");

        if cfg.include_trace {
            out.push_str("\n<trace>\n");
            out.push_str("  <note>trace not yet implemented; see task_tool_calls table</note>\n");
            out.push_str("</trace>\n");
        } else {
            out.push_str("\n<trace/>\n");
        }

        out
    }
}

fn push_block(out: &mut String, name: &str, content: &str) {
    out.push_str(&format!("<{}>\n", name));
    if !content.is_empty() {
        out.push_str(content);
        out.push('\n');
    }
    out.push_str(&format!("</{}>\n\n", name));
}

fn render_message(msg: &crate::bus::Message) -> String {
    let origin = msg.origin.as_str();
    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => format!(
            "<chat origin=\"{}\" from=\"{}\">{}</chat>",
            escape_attr(origin),
            escape_attr(&msg.sender),
            escape_text(t)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Request { task_id, head_id, goal, .. })) => format!(
            "<task origin=\"{}\" op=\"request\" id=\"{}\" head=\"{}\">{}</task>",
            escape_attr(origin),
            escape_attr(task_id),
            escape_attr(head_id),
            escape_text(goal)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Assigned { task_id, hand_id, .. })) => format!(
            "<task origin=\"{}\" op=\"assigned\" id=\"{}\" hand=\"{}\"/>",
            escape_attr(origin),
            escape_attr(task_id),
            escape_attr(hand_id)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Progress { task_id, hand_id, note })) => format!(
            "<task origin=\"{}\" op=\"progress\" id=\"{}\" hand=\"{}\">{}</task>",
            escape_attr(origin),
            escape_attr(task_id),
            escape_attr(hand_id),
            escape_text(note)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Result { task_id, hand_id, ok, summary })) => format!(
            "<task origin=\"{}\" op=\"result\" id=\"{}\" hand=\"{}\" ok=\"{}\">{}</task>",
            escape_attr(origin),
            escape_attr(task_id),
            escape_attr(hand_id),
            ok,
            escape_text(summary)
        ),
        _ => format!(
            "<msg origin=\"{}\" op=\"{}\" from=\"{}\"/>",
            escape_attr(origin),
            escape_attr(&format!("{:?}", msg.op)),
            escape_attr(&msg.sender),
        ),
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('\"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::bus::{Hub, Origin, Scope, respond};
    use crate::runtime::RuntimeBus;

    #[test]
    fn bundle_includes_layers_and_scopes() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        store.set_head_ltm("Monk", "LTM line").unwrap();
        store.set_head_stm("Monk", "STM line").unwrap();

        // Persist a couple messages for #general and a task.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let hub = Arc::new(RwLock::new(Hub::new()));
            let bus = RuntimeBus::new(hub, store.clone());
            bus.create_scope(Scope::from("#general")).await;
            bus.create_scope(Scope::Task("task/t-1".to_string())).await;

            bus.publish(respond::chat("alice", "#general", "hello").with_origin(Origin::Human)).await;
            bus.publish(
                respond::task_request("Monk", "§task/t-1", "t-1", "Monk", "do it", "steps: []")
                    .with_origin(Origin::Head),
            )
            .await;
        });

        let builder = HeadBundleBuilder::new(store);
        let cfg = HeadBundleConfig {
            head_id: "Monk".to_string(),
            include_scopes: vec![Scope::from("#general"), Scope::from("§task/t-1")],
            max_messages_per_scope: 10,
            include_trace: false,
            max_trace_steps: 0,
        };

        let bundle = builder.build(&cfg);
        assert!(bundle.contains("<grammar>"));
        assert!(bundle.contains("<system>"));
        assert!(bundle.contains("<ltm>"));
        assert!(bundle.contains("LTM line"));
        assert!(bundle.contains("<stm>"));
        assert!(bundle.contains("STM line"));
        assert!(bundle.contains("<focus>"));
        assert!(bundle.contains("<scope>"));
        assert!(bundle.contains("scope=\"#general\""));
        assert!(bundle.contains("<chat origin=\"human\" from=\"alice\">hello</chat>"));
        assert!(bundle.contains("scope=\"§task/t-1\""));
        assert!(bundle.contains("<task origin=\"head\" op=\"request\""));
        assert!(bundle.contains("<trace/>"));
    }
}
