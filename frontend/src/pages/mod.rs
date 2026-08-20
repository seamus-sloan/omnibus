//! Per-route page components.
//!
//! One submodule per top-level route in [`crate::Route`]. Each `pub use`
//! below re-exports the page component the router instantiates; flow logic
//! (data fetching, signal effects) lives inside the page modules.

mod account;
mod add_books;
#[cfg(not(feature = "mobile"))]
mod admin_health;
mod auth;
mod author;
mod authors_index;
mod book_detail;
mod check_in;
// Web/server only: the review page drives the admin-gated `cleanup/*` server
// functions; no mobile cleanup surface ships, same shape as `admin_health`.
#[cfg(not(feature = "mobile"))]
mod cleanup_review;
mod comic_reader;
mod index_shell;
mod landing;
mod listen;
#[cfg(not(feature = "mobile"))]
mod logs;
mod metadata_edit;
mod reader;
mod search;
#[cfg(feature = "mobile")]
mod search_mobile;
mod series;
mod series_index;
mod server_connect;
mod settings;
mod shelf_detail;
mod shelves_index;
mod stats;
mod tag_cloud;

pub use account::AccountPage;
pub use add_books::AddBooksPage;
#[cfg(not(feature = "mobile"))]
pub use admin_health::AdminHealthPage;
pub use auth::{LoginPage, RegisterPage};
pub use author::AuthorPage;
pub use authors_index::AuthorsIndexPage;
#[cfg(not(feature = "mobile"))]
pub(crate) use book_detail::retarget_and_open_immersive;
pub use book_detail::BookDetailPage;
pub use check_in::{CheckInOpen, CheckInOverlay, CheckInPage};
#[cfg(not(feature = "mobile"))]
pub use cleanup_review::CleanupReviewPage;
pub use comic_reader::ComicReadPage;
pub use landing::LandingPage;
#[cfg(feature = "web")]
pub(crate) use listen::install_audio_bootstrap;
pub use listen::BookListenPage;
#[cfg(not(feature = "mobile"))]
pub(crate) use listen::{dock_is_active, use_sleep_timer, AudioElement, MiniDock};
#[cfg(feature = "mobile")]
pub(crate) use listen::{mobile_dock_is_active, MobileAudioHost, MobileMiniPlayer, MobilePlayback};
#[cfg(not(feature = "mobile"))]
pub use logs::LogsPage;
pub use metadata_edit::MetadataEditPage;
pub use reader::BookReadPage;
pub use search::SearchPage;
#[cfg(feature = "mobile")]
pub use search_mobile::MobileSearchPage;
pub use series::SeriesPage;
pub use series_index::SeriesIndexPage;
pub use server_connect::ServerConnectPage;
pub use settings::SettingsPage;
pub use shelf_detail::ShelfDetailPage;
pub use shelves_index::ShelvesIndexPage;
pub use stats::StatsPage;
pub use tag_cloud::TagCloudPage;
