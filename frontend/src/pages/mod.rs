mod auth;
mod author;
mod book_detail;
mod landing;
mod metadata_edit;
mod series;
mod settings;
mod tag_cloud;

pub use auth::{LoginPage, RegisterPage};
pub use author::AuthorPage;
pub use book_detail::BookDetailPage;
pub use landing::LandingPage;
pub use metadata_edit::MetadataEditPage;
pub use series::SeriesPage;
pub use settings::SettingsPage;
pub use tag_cloud::TagCloudPage;
