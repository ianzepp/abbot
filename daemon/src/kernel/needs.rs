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

    pub async fn wait_for_need(&self) {
        self.notify.notified().await;
    }
}
