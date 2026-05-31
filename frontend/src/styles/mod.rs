//! App-wide CSS, split per page / per component to mirror
//! `frontend/src/pages/` and `frontend/src/components/`. `ALL_STYLES`
//! lists every submodule's CSS in the original source order so the
//! cascade (notably the duplicate `.format-badge` shared by the landing
//! table and the book-detail page) resolves identically.

pub mod auth;
pub mod book_detail;
pub mod bottom_nav;
pub mod globals;
pub mod landing;
pub mod settings;

/// Every CSS chunk injected by `App`, in source order. The renderer
/// emits one `<style>` tag per chunk; browsers concatenate them into a
/// single cascade so the result is equivalent to a single sheet.
pub const ALL_STYLES: &[&str] = &[
    globals::STYLES,
    auth::STYLES,
    bottom_nav::STYLES,
    settings::STYLES,
    landing::STYLES,
    book_detail::STYLES,
];
