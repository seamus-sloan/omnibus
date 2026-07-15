//! Filesystem storage for images embedded in journal entries (F3.2b).
//! Files live under `<journal_images_dir()>/<uuidv4>.<ext>` and are served by
//! `GET /api/journals/images/{name}`. Durable user data — not a regenerable
//! cache like thumbs/kepub — so nothing here evicts.

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

#[cfg(test)]
mod tests;
