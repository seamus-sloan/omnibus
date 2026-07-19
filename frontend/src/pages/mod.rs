//! Per-route page components.
//!
//! One submodule per top-level route in [`crate::Route`]. Each `pub use`
//! below re-exports the page component the router instantiates; flow logic
//! (data fetching, signal effects) lives inside the page modules.

mod account;
mod add_books;
mod auth;
mod author;
mod authors_index;
mod book_detail;
mod landing;
mod listen;
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
pub use auth::{LoginPage, RegisterPage};
pub use author::AuthorPage;
pub use authors_index::AuthorsIndexPage;
pub use book_detail::BookDetailPage;
pub use landing::LandingPage;
#[cfg(feature = "web")]
pub(crate) use listen::install_audio_bootstrap;
pub use listen::BookListenPage;
#[cfg(not(feature = "mobile"))]
pub(crate) use listen::{dock_is_active, use_sleep_timer, AudioElement, MiniDock};
#[cfg(feature = "mobile")]
pub(crate) use listen::{mobile_dock_is_active, MobileAudioHost, MobileMiniPlayer, MobilePlayback};
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
