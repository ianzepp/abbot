// Trace item types for organizing frames into hierarchy.

#[derive(Clone, Debug, PartialEq)]
pub enum TraceNodeType {
    Need,
    Task,
    Tool,
    Other,
}
