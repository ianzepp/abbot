use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static EFFECTIVE_BIND_ADDR: OnceLock<String> = OnceLock::new();

pub fn set_effective_bind_addr(addr: impl Into<String>) {
    let _ = EFFECTIVE_BIND_ADDR.set(addr.into());
}

pub fn effective_bind_addr() -> Option<&'static str> {
    EFFECTIVE_BIND_ADDR.get().map(|s| s.as_str())
}

// ---------------------------------------------------------------------------
// Service disable flags (set once at startup via --no-hand, --no-mind, --no-need)
// ---------------------------------------------------------------------------

static HAND_DISABLED: AtomicBool = AtomicBool::new(false);
static MIND_DISABLED: AtomicBool = AtomicBool::new(false);
static NEED_DISABLED: AtomicBool = AtomicBool::new(false);

pub fn set_hand_disabled(v: bool) {
    HAND_DISABLED.store(v, Ordering::Relaxed);
}
pub fn hand_disabled() -> bool {
    HAND_DISABLED.load(Ordering::Relaxed)
}

pub fn set_mind_disabled(v: bool) {
    MIND_DISABLED.store(v, Ordering::Relaxed);
}
pub fn mind_disabled() -> bool {
    MIND_DISABLED.load(Ordering::Relaxed)
}

pub fn set_need_disabled(v: bool) {
    NEED_DISABLED.store(v, Ordering::Relaxed);
}
pub fn need_disabled() -> bool {
    NEED_DISABLED.load(Ordering::Relaxed)
}
