//! Feature-gated data-fetching layer: mobile calls the server's
//! hand-written `/api/*` REST routes via `reqwest`; web and server-only
//! compiles call the `#[get]`/`#[post]` functions in [`crate::rpc`].
//! Per-domain wrappers live in the [`auth`], [`authors`], [`books`], and
//! sibling submodules, re-exported here; [`error`], [`transport`], and the
//! mobile-only `stores` carry the shared error type, HTTP plumbing, and
//! on-device persistence they build on.

mod account;
// Admin server-health report (F5, #952) — web (server-fn) + SSR stubs; not
// a mobile surface, same shape as `errors`/`logs`.
#[cfg(not(feature = "mobile"))]
mod admin_health;
// Admin device & session management (F5.4, #910) — web (gloo REST) + SSR
// stubs; no mobile surface.
#[cfg(not(feature = "mobile"))]
mod admin_sessions;
mod auth;
mod authors;
// Admin background-task history read (#941) — web (server-fn) + SSR stubs;
// not a mobile surface, same shape as `errors`.
#[cfg(not(feature = "mobile"))]
mod background_tasks;
mod bookmarks;
mod books;
// Admin library-cleanup review (#966). Web/SSR call the server functions;
// mobile posts the same `/api/rpc/cleanup/*` routes, so the wrappers compile
// under both gates even though no mobile UI ships yet.
mod cleanup;
// Admin "last errors" ring buffer read (#954) — web (server-fn) + SSR stubs;
// not a mobile surface, same shape as `logs`.
#[cfg(not(feature = "mobile"))]
mod errors;
mod genres;
mod hardcover_fetch;
mod highlights;
// Per-book Started/Time-read/Sessions insights (#1904) — the book-detail
// rail section that renders them doesn't compile on mobile, so there's no
// REST transport, same shape as `admin_health`/`errors`.
#[cfg(not(feature = "mobile"))]
mod insights;
mod journals;
mod kindle;
// Per-user Kobo device tokens (#923) — web (server-fn) + SSR stubs; the Account
// settings card is web-only, so there's no mobile transport.
mod cross_format;
#[cfg(not(feature = "mobile"))]
mod kobo;
#[cfg(not(feature = "mobile"))]
mod logs;
mod physical;
mod profile;
mod progress;
mod ratings;
mod read_status;
mod scan;
mod series;
// Self-service session management (F5.4, #910) — web (gloo REST) + SSR
// stubs; no mobile surface yet.
#[cfg(not(feature = "mobile"))]
mod sessions;
mod shelves;
mod stats;
mod suggestions;
mod summary;
mod tags;
mod uploads;
// Admin user management (F5.4) — web (gloo REST) + SSR stubs; no mobile surface.
#[cfg(not(feature = "mobile"))]
mod users;

mod error;
#[cfg(feature = "mobile")]
mod stores;
mod transport;
// auth exports exist under web, mobile, and server-only (the last only
// re-exports the SSR `current_user` stub so pages can call `data::current_user`
// unconditionally without diverging hook order between SSR and WASM).
pub use account::*;
#[cfg(not(feature = "mobile"))]
pub use admin_health::*;
#[cfg(not(feature = "mobile"))]
pub use admin_sessions::*;
#[cfg(any(feature = "web", feature = "mobile", feature = "server"))]
pub use auth::*;
pub use authors::*;
#[cfg(not(feature = "mobile"))]
pub use background_tasks::*;
pub use bookmarks::*;
pub use books::*;
pub use cleanup::*;
pub use cross_format::*;
#[cfg(not(feature = "mobile"))]
pub use errors::*;
pub use genres::*;
pub use hardcover_fetch::*;
pub use highlights::*;
#[cfg(not(feature = "mobile"))]
pub use insights::*;
pub use journals::*;
pub use kindle::*;
#[cfg(not(feature = "mobile"))]
pub use kobo::*;
#[cfg(not(feature = "mobile"))]
pub use logs::*;
pub use physical::*;
pub use profile::*;
pub use progress::*;
pub use ratings::*;
pub use read_status::*;
pub use scan::*;
pub use series::*;
#[cfg(not(feature = "mobile"))]
pub use sessions::*;
pub use shelves::*;
pub use stats::*;
pub use suggestions::*;
pub use summary::*;
pub use tags::*;
pub use uploads::*;
#[cfg(not(feature = "mobile"))]
pub use users::*;

pub use error::{server_error_message, DataError};
#[cfg(feature = "mobile")]
pub use stores::{app_dirs, server_url_store, token_store, ServerUrl};
#[cfg(not(feature = "mobile"))]
pub(crate) use transport::note_server_fn_err;
#[cfg(feature = "web")]
pub use transport::web_auth_state;
#[cfg(feature = "mobile")]
pub(crate) use transport::{
    client_kind, drain_error, http_client, note_status, require_online, streaming_client,
    with_bearer,
};
