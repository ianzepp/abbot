use tokio::sync::Notify;

#[derive(Debug, Default)]
pub struct NeedKernel {
    notify: Notify,
}

impl NeedKernel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn notify_enqueue(&self) {
        self.notify.notify_one();
    }

    /// Return a future that completes on the next notify_one().
    /// Register this BEFORE checking the condition to avoid lost wakeups.
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}
