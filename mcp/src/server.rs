//! The MCP service: one [`OmnibusMcp`] per process, serving the combined
//! tool router over stdio. Tool implementations live under [`crate::tools`],
//! one `#[tool_router(router = …)]` impl block per family; this module owns
//! the struct, the router combination, and the `ServerHandler` glue.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool_handler, ErrorData, ServerHandler};

use crate::client::{ClientError, OmnibusClient};

/// The MCP server: a shared authenticated client plus the tool router.
#[derive(Clone)]
pub struct OmnibusMcp {
    pub(crate) client: Arc<OmnibusClient>,
    tool_router: ToolRouter<Self>,
}

impl OmnibusMcp {
    /// Build the service around an authenticated client.
    ///
    /// Later issues add write-tool families as further
    /// `#[tool_router(router = <name>)]` impl blocks under `crate::tools`
    /// and combine them here: `Self::read_tools() + Self::write_tools()`.
    pub fn new(client: Arc<OmnibusClient>) -> Self {
        Self {
            client,
            tool_router: Self::read_tools(),
        }
    }
}

/// Tool failures surface as MCP errors; `ClientError`'s messages carry no
/// credentials (the password and token never appear in any error `Display`).
impl From<ClientError> for ErrorData {
    fn from(e: ClientError) -> Self {
        ErrorData::internal_error(e.to_string(), None)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OmnibusMcp {
    fn get_info(&self) -> ServerInfo {
        // Not `Implementation::from_build_env()` alone — that reports the
        // rmcp crate's own name/version, not this binary's (the struct is
        // non-exhaustive, so mutate its fields instead).
        let mut implementation = Implementation::from_build_env();
        implementation.name = env!("CARGO_PKG_NAME").to_string();
        implementation.version = env!("CARGO_PKG_VERSION").to_string();
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(implementation)
            .with_instructions(
                "Read-only access to an Omnibus ebook/audiobook library: browse and \
                 search books, explore authors/series/tags/genres and shelves, and read \
                 the signed-in user's stats, progress, highlights, bookmarks, and \
                 journal entries. Books are identified by the uuid field returned by \
                 the listing and search tools. No tool mutates the library."
                    .to_string(),
            )
    }
}

#[cfg(test)]
mod tests;
