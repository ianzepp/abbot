// HTTP API for external clients to interact with the bus.
//
// The API provides a simple request/response protocol over HTTP for operations
// like publishing chat messages and creating tasks. The server runs as a
// background task alongside the main bus. Clients (CLI, external integrations)
// use ApiClient to communicate with the running server.

mod proto;
mod server;
mod client;

pub use client::ApiClient;
pub use proto::{ApiRequest, ApiResponse};
pub use server::ApiServer;

