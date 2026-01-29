use std::sync::Arc;

use crate::bus::{MessageData, MessageOp, Scope, TaskMsg};
use crate::history::Store;

pub struct HandBundleConfig {
    pub task_scope: Scope,
    pub max_task_messages: usize,
    pub include_trace: bool,
    pub max_trace_steps: usize,
}

impl HandBundleConfig {
    pub fn for_task(task_id: &str) -> Self {
        Self {
            task_scope: Scope::Task(format!("task/{}", task_id)),
            max_task_messages: 80,
            include_trace: false,
            max_trace_steps: 20,
        }
    }
}

pub struct HandBundleBuilder {
    store: Arc<Store>,
}

impl HandBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn build(&self, cfg: &HandBundleConfig) -> String {
        let grammar = include_str!("../grammar/hand.md");

        let mut out = String::new();
        push_block(&mut out, "grammar", grammar.trim());

        let focus = format!("task_scope={}", cfg.task_scope);
        push_block(&mut out, "focus", focus.trim());

        out.push_str("\n<scope>\n");
        let scope_str = cfg.task_scope.to_string();
        let messages = self
            .store
            .recent(&scope_str, cfg.max_task_messages)
            .unwrap_or_default();

        out.push_str(&format!("  <stream scope=\"{}\">\n", escape_attr(&scope_str)));
        for m in messages {
            out.push_str("    ");
            out.push_str(&render_task_message(&m));
            out.push('\n');
        }
        out.push_str("  </stream>\n");
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

fn render_task_message(msg: &crate::bus::Message) -> String {
    match (&msg.op, &msg.data) {
        (MessageOp::Task, MessageData::Task(TaskMsg::Request { task_id, head_id, goal, .. })) => format!(
            "<task op=\"request\" id=\"{}\" head=\"{}\">{}</task>",
            escape_attr(task_id),
            escape_attr(head_id),
            escape_text(goal)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Assigned { task_id, hand_id, .. })) => format!(
            "<task op=\"assigned\" id=\"{}\" hand=\"{}\"/>",
            escape_attr(task_id),
            escape_attr(hand_id)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Progress { task_id, hand_id, note })) => format!(
            "<task op=\"progress\" id=\"{}\" hand=\"{}\">{}</task>",
            escape_attr(task_id),
            escape_attr(hand_id),
            escape_text(note)
        ),
        (MessageOp::Task, MessageData::Task(TaskMsg::Result { task_id, hand_id, ok, summary })) => format!(
            "<task op=\"result\" id=\"{}\" hand=\"{}\" ok=\"{}\">{}</task>",
            escape_attr(task_id),
            escape_attr(hand_id),
            ok,
            escape_text(summary)
        ),
        _ => format!(
            "<msg op=\"{}\" from=\"{}\"/>",
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
    use tokio::sync::RwLock;

    use crate::bus::{Hub, respond};
    use crate::runtime::RuntimeBus;

    #[test]
    fn bundle_includes_grammar_focus_scope() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let hub = Arc::new(RwLock::new(Hub::new()));
            let bus = RuntimeBus::new(hub, store.clone());

            let scope = Scope::Task("task/t-2".to_string());
            bus.create_scope(scope.clone()).await;
            bus.publish(respond::task_request("Monk", scope.clone(), "t-2", "Monk", "do it", "steps: []")).await;
        });

        let builder = HandBundleBuilder::new(store);
        let cfg = HandBundleConfig::for_task("t-2");
        let bundle = builder.build(&cfg);

        assert!(bundle.contains("<grammar>"));
        assert!(bundle.contains("<focus>"));
        assert!(bundle.contains("task_scope=§task/t-2"));
        assert!(bundle.contains("<scope>"));
        assert!(bundle.contains("scope=\"§task/t-2\""));
        assert!(bundle.contains("<task op=\"request\""));
        assert!(bundle.contains("<trace/>"));
    }
}

