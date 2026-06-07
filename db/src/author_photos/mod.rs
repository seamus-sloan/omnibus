//! Author profile photo pipeline. Resolves the manual → Open Library →
//! letter cascade for a given author and persists the result. The SSRF-
//! guarded remote-image fetch for admin "paste URL" uploads lives in
//! [`remote`]. Driven by [`crate::worker::Task::ResolveAuthorPhoto`].

mod shared;

pub mod cascade;
pub mod remote;

pub use cascade::{refetch_all, resolve, resolve_with, OpenLibraryConfig};
pub use remote::{
    fetch_remote_image, fetch_remote_image_with, FetchRemoteImageError, RemoteImageConfig,
    REMOTE_IMAGE_MAX_BYTES,
};

#[cfg(test)]
mod tests;
