//! Stop command - Stop the daemon (thin wrapper around service::stop_service)

use crate::error::CliError;
use crate::output::OutputFormat;

pub async fn run(format: OutputFormat) -> Result<(), CliError> {
    super::service::stop_service(format).await
}
