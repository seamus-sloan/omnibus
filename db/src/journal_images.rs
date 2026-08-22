//! Filesystem storage for images embedded in journal entries. Files live under
//! `<journal_images_dir()>/<uuidv4>.<ext>` and are served by
//! `GET /api/journals/images/{name}`. Durable user data rather than a
//! regenerable cache, so there is no capacity eviction; orphans are swept
//! best-effort by `journals::{update_journal_entry, delete_journal_entry}`.

use std::collections::HashSet;
use std::path::PathBuf;

/// Root directory for journal images.
///
/// Override with `$OMNIBUS_JOURNAL_IMAGES_DIR` (used verbatim); otherwise
/// defaults to `<$OMNIBUS_DATA_DIR>/journal-images` (data dir default
/// `./data`).
pub fn journal_images_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIBUS_JOURNAL_IMAGES_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("journal-images")
}

/// Persist an uploaded journal image and return its generated file name
/// (`<uuidv4>.<ext>`). The name is minted server-side so a client can never
/// influence the on-disk path. Sync `std::fs` — call from `spawn_blocking`.
pub fn write_journal_image(mime: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        other => anyhow::bail!("unsupported journal image mime: {other}"),
    };
    let dir = journal_images_dir();
    std::fs::create_dir_all(&dir)?;
    let name = format!("{}.{ext}", uuid::Uuid::new_v4());
    std::fs::write(dir.join(&name), bytes)?;
    Ok(name)
}

/// Read a stored journal image by its serving name, returning `(mime, bytes)`.
/// `None` covers both an invalid name and a missing file — the caller 404s
/// either way. Sync `std::fs` — call from `spawn_blocking`.
pub fn read_journal_image(name: &str) -> Option<(&'static str, Vec<u8>)> {
    if !is_valid_image_name(name) {
        return None;
    }
    let mime = match name.rsplit('.').next() {
        Some("jpg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(journal_images_dir().join(name)).ok()?;
    Some((mime, bytes))
}

/// Whether `name` has the exact `<uuidv4>.<ext>` shape we mint — the
/// path-traversal guard for the read path (no separators, no dots beyond the
/// single extension, uuid segment parses).
fn is_valid_image_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else {
        return false;
    };
    matches!(ext, "jpg" | "png" | "gif" | "webp") && uuid::Uuid::parse_str(stem).is_ok()
}

/// Every journal-image serving name referenced in `body_md` via the
/// `IMAGE_URL_PREFIX` embed URL (`journals::markdown::IMAGE_URL_PREFIX`), for
/// orphan-cleanup diffing. Malformed matches (a prefix not followed by a
/// well-formed `<uuidv4>.<ext>` name) are ignored rather than collected.
pub fn referenced_image_names(body_md: &str) -> HashSet<String> {
    let prefix = crate::journals::markdown::IMAGE_URL_PREFIX;
    let mut names = HashSet::new();
    let mut rest = body_md;
    while let Some(pos) = rest.find(prefix) {
        let after = &rest[pos + prefix.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
            .unwrap_or(after.len());
        let name = &after[..end];
        if is_valid_image_name(name) {
            names.insert(name.to_string());
        }
        rest = &after[end..];
    }
    names
}

/// Best-effort delete of a stored journal image by its serving name. A
/// missing file and a malformed name are both no-ops; a real I/O error is
/// logged, not propagated — this is opportunistic orphan GC, not a
/// correctness-critical path. Sync `std::fs` — call from `spawn_blocking`.
pub fn delete_journal_image(name: &str) {
    if !is_valid_image_name(name) {
        return;
    }
    if let Err(e) = std::fs::remove_file(journal_images_dir().join(name)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(name, error = %e, "failed to delete orphaned journal image");
        }
    }
}

#[cfg(test)]
mod tests;
