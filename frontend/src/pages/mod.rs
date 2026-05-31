//! Per-route page components.
//!
//! One submodule per top-level route in [`crate::Route`]. Each `pub use`
//! below re-exports the page component the router instantiates; flow logic
//! (data fetching, signal effects) lives inside the page modules.

mod auth;
mod author;
mod authors_index;
mod book_detail;
mod landing;
mod listen;
mod metadata_edit;
mod reader;
mod search;
mod series;
mod series_index;
mod settings;
mod tag_cloud;

pub use auth::{LoginPage, RegisterPage};
pub use author::AuthorPage;
pub use authors_index::AuthorsIndexPage;
pub use book_detail::BookDetailPage;
pub use landing::LandingPage;
pub use listen::BookListenPage;
pub use metadata_edit::MetadataEditPage;
pub use reader::BookReadPage;
pub use search::SearchPage;
pub use series::SeriesPage;
pub use series_index::SeriesIndexPage;
pub use settings::SettingsPage;
pub use tag_cloud::TagCloudPage;
