//! Restart command - Stop then start the daemon

use crate::error::CliError;
use crate::output::OutputFormat;

pub async fn run(format: OutputFormat) -> Result<(), CliError> {
    super::service::stop_service(format).await?;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    super::service::start_service(format).await
}
