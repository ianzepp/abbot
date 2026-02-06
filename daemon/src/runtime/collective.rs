use std::sync::atomic::{AtomicU64, Ordering};

static REBOOT_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn reboot_epoch() -> u64 {
    REBOOT_EPOCH.load(Ordering::SeqCst)
}

pub fn bump_reboot_epoch() -> u64 {
    REBOOT_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn rebooted_since(epoch: u64) -> bool {
    reboot_epoch() != epoch
}
