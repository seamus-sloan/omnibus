//! Physical Check-In scan resolution: map a scanned/typed ISBN down the
//! matching ladder to a [`ScanOutcome`], and compose the write ops for each
//! branch (check in, add a physical-only book, wishlist). The library-write
//! primitives live in [`crate::physical`]; this module is the resolution brain
//! plus the fileless-from-external-metadata composition.

mod resolve;

#[cfg(test)]
mod tests;

pub use resolve::{add_physical_only, resolve_meta, resolve_scan, wishlist_add, ScanError};
