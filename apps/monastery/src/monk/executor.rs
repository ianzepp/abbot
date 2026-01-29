use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use crate::bus::{Hub, respond};
use crate::history::Store;
use crate::tools::{
    ExecutionContext, Tool, SharedCwd,
    BashTool, ReadTool, WriteTool, EditTool, FindTool, DiffTool,
    MonkTool, ChannelTool, SelfTool, WorkspaceTool, PrayTool, GardenTool, PetitionTool,
};
use super::{ParsedResponse, Action, SharedRegistry};

/// Result from executing a single tool.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool: String,
    pub content: String,
    pub output: String,
    pub success: bool,
}

/// Result from executing all actions in a response.
#[derive(Debug, Clone, Default)]
pub struct ExecutionResult {
    pub tool_results: Vec<ToolResult>,
    pub messages_sent: Vec<(String, String)>,
}

impl ExecutionResult {
    pub fn is_empty(&self) -> bool {
        self.tool_results.is_empty() && self.messages_sent.is_empty()
    }

    /// Format tool results as XML for feeding back to LLM.
    pub fn format_for_llm(&self) -> String {
        if self.tool_results.is_empty() {
            return String::new();
        }

        self.tool_results
            .iter()
            .map(|r| {
                format!(
                    r#"<tool_result tool="{}" success="{}">
{}
</tool_result>"#,
                    r.tool,
                    r.success,
                    r.output.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Executor for parsed LLM responses.
pub struct Executor {
    hub: Arc<RwLock<Hub>>,
    store: Arc<Store>,
    registry: SharedRegistry,
    tools: Vec<Box<dyn Tool>>,
}

impl Executor {
    pub fn new(hub: Arc<RwLock<Hub>>, store: Arc<Store>, registry: SharedRegistry) -> Self {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(BashTool),
            Box::new(ReadTool),
            Box::new(WriteTool),
            Box::new(EditTool),
            Box::new(FindTool),
            Box::new(DiffTool),
            Box::new(MonkTool::new()),
            Box::new(ChannelTool::new()),
            Box::new(SelfTool::new()),
            Box::new(WorkspaceTool::new()),
            Box::new(PrayTool::new()),
            Box::new(GardenTool::new()),
            Box::new(PetitionTool::new()),
        ];

        Self {
            hub,
            store,
            registry,
            tools,
        }
    }

    /// Execute all actions from a parsed response.
    pub async fn execute(
        &self,
        parsed: &ParsedResponse,
        monk_id: &str,
        channel: &str,
        cwd: SharedCwd,
        batch_id: &str,
        iteration: usize,
    ) -> ExecutionResult {
        let mut result = ExecutionResult::default();

        // Build execution context
        let ctx = ExecutionContext {
            cwd,
            sender: monk_id.to_string(),
            channel: channel.to_string(),
            store: self.store.clone(),
            registry: self.registry.clone(),
            hub: self.hub.clone(),
        };

        // Collect exec and say actions
        let mut exec_futures = Vec::new();
        let mut say_actions = Vec::new();
        let mut position = 0usize;

        for action in &parsed.actions {
            match action {
                Action::Exec { tool, reason, destructive, content } => {
                    let tool_name = tool.clone();
                    let current_position = position;
                    position += 1;
                    let tool_reason = reason.clone();
                    let tool_destructive = *destructive;
                    let tool_content = content.clone();

                    if let Some(tool_impl) = self.find_tool(&tool_name) {
                        let ctx_clone = ctx.clone();
                        let store_clone = self.store.clone();
                        let monk_id = monk_id.to_string();
                        let batch_id = batch_id.to_string();
                        let tool_name_log = tool_name.clone();
                        let tool_reason_log = tool_reason.clone();
                        exec_futures.push(async move {
                            tracing::info!(
                                tool = %tool_name_log,
                                reason = ?tool_reason_log,
                                destructive = tool_destructive,
                                "tool executing"
                            );
                            let start = std::time::Instant::now();
                            let output = tool_impl.execute(&tool_content, &ctx_clone).await;
                            let duration_ms = start.elapsed().as_millis() as u64;
                            tracing::debug!(tool = %tool_name_log, duration_ms, "tool finished");

                            // Log to database
                            if let Err(e) = store_clone.log_tool_call(
                                &monk_id,
                                &batch_id,
                                iteration,
                                current_position,
                                &tool_name_log,
                                tool_reason_log.as_deref(),
                                &tool_content,
                                &output,
                                true,
                                duration_ms,
                            ) {
                                tracing::warn!(error = %e, "failed to log tool call");
                            }

                            ToolResult {
                                tool: tool_name,
                                content: tool_content,
                                output,
                                success: true,
                            }
                        });
                    } else {
                        result.tool_results.push(ToolResult {
                            tool: tool_name,
                            content: tool_content,
                            output: "error: unknown tool".to_string(),
                            success: false,
                        });
                    }
                }
                Action::Say { channel, text } => {
                    say_actions.push((channel.clone(), text.clone()));
                }
            }
        }

        // Execute all tools concurrently
        let tool_results = futures::future::join_all(exec_futures).await;
        result.tool_results.extend(tool_results);

        // Execute say actions (publish to channels)
        for (target_channel, text) in say_actions {
            let msg = respond::chat(monk_id, &target_channel, &text);

            // Persist to database
            if let Err(e) = self.store.insert(&msg) {
                tracing::warn!(error = %e, "failed to persist message");
            }

            self.hub.read().await.publish(&target_channel, msg);
            result.messages_sent.push((target_channel, text));
        }

        result
    }

    fn find_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monk::{parse, new_registry};
    use crate::bus::Hub;
    use std::path::PathBuf;

    fn test_setup() -> (Arc<RwLock<Hub>>, Arc<Store>, SharedRegistry) {
        let hub = Arc::new(RwLock::new(Hub::new()));
        let store = Arc::new(Store::open(":memory:").unwrap());
        let registry = new_registry();
        (hub, store, registry)
    }

    fn test_cwd() -> SharedCwd {
        Arc::new(Mutex::new(PathBuf::from("/tmp")))
    }

    #[tokio::test]
    async fn test_executor_empty() {
        let (hub, store, registry) = test_setup();
        let executor = Executor::new(hub, store, registry);

        let parsed = parse("");
        let result = executor.execute(&parsed, "test-monk", "#general", test_cwd(), "test-batch", 0).await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_executor_bash() {
        let (hub, store, registry) = test_setup();
        let executor = Executor::new(hub, store, registry);

        let parsed = parse(r#"<exec tool="bash">echo hello</exec>"#);
        let result = executor.execute(&parsed, "test-monk", "#general", test_cwd(), "test-batch", 0).await;

        assert_eq!(result.tool_results.len(), 1);
        assert_eq!(result.tool_results[0].tool, "bash");
        assert!(result.tool_results[0].output.contains("hello"));
        assert!(result.tool_results[0].success);
    }

    #[tokio::test]
    async fn test_executor_unknown_tool() {
        let (hub, store, registry) = test_setup();
        let executor = Executor::new(hub, store, registry);

        let parsed = parse(r#"<exec tool="notreal">anything</exec>"#);
        let result = executor.execute(&parsed, "test-monk", "#general", test_cwd(), "test-batch", 0).await;

        assert_eq!(result.tool_results.len(), 1);
        assert!(!result.tool_results[0].success);
        assert!(result.tool_results[0].output.contains("unknown tool"));
    }

    #[tokio::test]
    async fn test_executor_say() {
        let (hub, store, registry) = test_setup();

        // Create the channel first
        hub.write().await.create_channel("#general");

        let executor = Executor::new(hub, store, registry);

        let parsed = parse(r##"<say channel="#general">Hello world</say>"##);
        let result = executor.execute(&parsed, "test-monk", "#general", test_cwd(), "test-batch", 0).await;

        assert_eq!(result.messages_sent.len(), 1);
        assert_eq!(result.messages_sent[0].0, "#general");
        assert_eq!(result.messages_sent[0].1, "Hello world");
    }

    #[tokio::test]
    async fn test_executor_parallel() {
        let (hub, store, registry) = test_setup();
        hub.write().await.create_channel("#general");

        let executor = Executor::new(hub, store, registry);

        let response = r##"<exec tool="bash">echo one</exec>
<exec tool="bash">echo two</exec>
<say channel="#general">Hello</say>"##;

        let parsed = parse(response);
        let result = executor.execute(&parsed, "test-monk", "#general", test_cwd(), "test-batch", 0).await;

        assert_eq!(result.tool_results.len(), 2);
        assert_eq!(result.messages_sent.len(), 1);
    }

    #[tokio::test]
    async fn test_format_for_llm() {
        let result = ExecutionResult {
            tool_results: vec![
                ToolResult {
                    tool: "bash".to_string(),
                    content: "echo hello".to_string(),
                    output: "hello".to_string(),
                    success: true,
                },
                ToolResult {
                    tool: "read".to_string(),
                    content: "/tmp/test.txt".to_string(),
                    output: "file contents".to_string(),
                    success: true,
                },
            ],
            messages_sent: vec![],
        };

        let formatted = result.format_for_llm();
        assert!(formatted.contains(r#"<tool_result tool="bash" success="true">"#));
        assert!(formatted.contains("hello"));
        assert!(formatted.contains(r#"<tool_result tool="read" success="true">"#));
        assert!(formatted.contains("file contents"));
    }
}
