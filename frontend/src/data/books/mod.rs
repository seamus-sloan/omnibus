//! Book, library, search, settings, overrides, and worker fetchers. Each
//! function has a mobile REST variant (`reqwest`) and a web/SSR variant (a
//! Dioxus server-function wrapper) with identical signatures across the
//! `#[cfg]` split. Split by sub-topic: [`admin`], [`library`] (browse, search,
//! manifests, validators), and [`manage`] (overrides, covers, merge, delete).

mod admin;
mod library;
mod manage;

pub use admin::*;
pub use library::*;
pub use manage::*;

#[cfg(all(test, feature = "mobile"))]
mod tests;
