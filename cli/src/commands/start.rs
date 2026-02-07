//! Start command - Start the daemon (thin wrapper around service::start_service)

use crate::error::CliError;
use crate::output::OutputFormat;

pub async fn run(format: OutputFormat) -> Result<(), CliError> {
    super::service::start_service(format).await
}
