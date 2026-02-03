use crate::ems::ems_tool_specs;
use crate::llm::ToolSpec;

pub fn head_specs() -> Vec<ToolSpec> {
    let mut specs = ems_tool_specs();
    for s in &mut specs {
        s.function.name = format!("head__{}", s.function.name);
    }
    specs
}

pub fn hand_specs() -> Vec<ToolSpec> {
    let ems_readonly = ["ems_query", "ems_select", "ems_describe"];

    let mut specs = ems_tool_specs();
    specs.retain(|t| ems_readonly.contains(&t.function.name.as_str()));
    for s in &mut specs {
        s.function.name = format!("hand__{}", s.function.name);
    }
    specs
}
