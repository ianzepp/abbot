use std::sync::OnceLock;

static EFFECTIVE_BIND_ADDR: OnceLock<String> = OnceLock::new();

pub fn set_effective_bind_addr(addr: impl Into<String>) {
    let _ = EFFECTIVE_BIND_ADDR.set(addr.into());
}

pub fn effective_bind_addr() -> Option<&'static str> {
    EFFECTIVE_BIND_ADDR.get().map(|s| s.as_str())
}
