#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Immediate,
    Need,
    Task,
    Room,
}

#[derive(Debug, Default, Clone)]
pub struct KernelRouter;

impl KernelRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn lane_for(&self, syscall_name: &str) -> Lane {
        if syscall_name.starts_with("task:") {
            return Lane::Task;
        }
        if syscall_name.starts_with("need:") {
            return Lane::Need;
        }
        if syscall_name.starts_with("room:") {
            return Lane::Room;
        }
        if syscall_name.starts_with("mind:convene_") {
            return Lane::Room;
        }
        if syscall_name == "mind:consult" {
            return Lane::Room;
        }
        Lane::Immediate
    }
}
