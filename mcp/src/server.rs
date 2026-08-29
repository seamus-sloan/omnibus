//! The MCP service: one [`OmnibusMcp`] per process, serving the combined
//! tool router over stdio. Tool implementations live under [`crate::tools`],
//! one `#[tool_router(router = …)]` impl block per family; this module owns
//! the struct, the router combination, and the `ServerHandler` glue.

use std::sync::{Arc, OnceLock};

use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Icon, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool_handler, ErrorData, ServerHandler};

use crate::client::{ClientError, OmnibusClient};

/// The Omnibus stoat mark, embedded at compile time. A `data:` URI is the one
/// icon source that works on both transports: stdio has no origin to serve
/// from, and `/mcp`'s bearer gate would block a client UI's anonymous fetch.
const STOAT_PNG: &[u8] = include_bytes!("../../frontend/assets/omnibus-stoat.png");

/// The server icon advertised in `initialize`'s `serverInfo.icons`, encoded
/// once per process.
fn stoat_icon() -> Icon {
    static DATA_URI: OnceLock<String> = OnceLock::new();
    let src = DATA_URI.get_or_init(|| {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(STOAT_PNG)
        )
    });
    Icon::new(src.clone())
        .with_mime_type("image/png")
        .with_sizes(vec!["128x128".to_string()])
}

/// The MCP server: a shared authenticated client plus the tool router.
#[derive(Clone)]
pub struct OmnibusMcp {
    pub(crate) client: Arc<OmnibusClient>,
    tool_router: ToolRouter<Self>,
}

impl OmnibusMcp {
    /// Build the service around an authenticated client.
    ///
    /// Later issues add further tool families as
    /// `#[tool_router(router = <name>)]` impl blocks under `crate::tools`
    /// and combine them here.
    pub fn new(client: Arc<OmnibusClient>) -> Self {
        Self {
            client,
            tool_router: Self::read_tools()
                + Self::checkin_tools()
                + Self::shelf_tools()
                + Self::metadata_tools()
                + Self::merge_tools()
                + Self::content_tools(),
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
        implementation.icons = Some(vec![stoat_icon()]);
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(implementation)
            .with_instructions(
                "Access to an Omnibus ebook/audiobook library. Read-only tools browse \
                 and search books, explore authors/series/tags/genres and shelves, and \
                 read the signed-in user's stats, progress, highlights, bookmarks, and \
                 journal entries; books are identified by the uuid field returned by \
                 the listing and search tools. Book text is readable too: list_chapters \
                 maps a book's chapters to spine indexes, read_chapter_text reads one \
                 chapter as bounded plain-text slices (page via next_offset), and \
                 search_book_content full-text-searches the library's book text — \
                 distinct from search_books, which matches metadata only — citing each \
                 hit back to a book and chapter. The physical-collection tools resolve \
                 ISBNs and title searches against the library and the external \
                 metadata providers (always relay which provider answered), check in \
                 physical copies, and manage the wishlist and copy notes; \
                 check_in_physical_book and remove_physical_copy have lasting effects, \
                 so they require confirm=true — resolve first, show the user the \
                 target, and confirm only with their approval. The shelf tools author \
                 shelves for the signed-in user: iterate a smart rule with \
                 preview_shelf_rule (which creates nothing) before create_shelf, and \
                 delete_shelf requires confirm=true after explicit user confirmation. \
                 Metadata repair follows a dry-run workflow: propose_metadata_changes \
                 computes a per-book before/after diff without writing; \
                 apply_metadata_changes and revert_metadata_overrides write metadata \
                 overrides — library-wide state every user sees — and refuse to run \
                 until called with confirm: true after the user has approved the diff. \
                 Duplicate resolution is admin-only and the strongest write here: \
                 merge_books deletes the source book's row and retargets every reader's \
                 state onto the target, undo_merge reverses a merge via its returned \
                 merge_log_id, and both refuse to run without confirm: true — fetch both \
                 books with get_book and present them to the user first. Settings and \
                 reading state are never modified."
                    .to_string(),
            )
    }
}

#[cfg(test)]
mod tests;
