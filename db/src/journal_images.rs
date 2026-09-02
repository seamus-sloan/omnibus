//! Filesystem storage for images embedded in journal entries. Files live under
//! `<journal_images_dir()>/<uuidv4>.<ext>` and are served by
//! `GET /api/journals/images/{name}`. Durable user data rather than a
//! regenerable cache, so there is no capacity eviction; orphans are swept
//! best-effort by `journals::{update_journal_entry, delete_journal_entry}`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Root directory for journal images.
///
/// Override with `$OMNIBUS_JOURNAL_IMAGES_DIR` (used verbatim); otherwise
/// defaults to `<$OMNIBUS_DATA_DIR>/journal-images` (data dir default
/// `./data`).
pub fn journal_images_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIBUS_JOURNAL_IMAGES_DIR") {
        return PathBuf::from(dir);
    }
    data_dir_journal_images()
}

/// The `$OMNIBUS_DATA_DIR` default location — where images land when
/// `OMNIBUS_JOURNAL_IMAGES_DIR` is unset, and where every Docker instance
/// predating that variable's image default put them.
fn data_dir_journal_images() -> PathBuf {
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("journal-images")
}

/// Move journal images sitting in the `$OMNIBUS_DATA_DIR` default into the
/// directory `OMNIBUS_JOURNAL_IMAGES_DIR` now names. Call once at boot; a
/// no-op when the variable is unset or already names that same directory.
///
/// The Docker image left the variable unset, so images landed in `/cache` —
/// the volume operators are told they may delete. Relocation is best-effort
/// and never fails the boot: a name already present at the destination is
/// left on both sides rather than overwritten, a name we don't mint is left
/// alone, and a cross-volume `rename` falls back to copy-then-unlink.
pub fn relocate_legacy_journal_images() {
    let dest = journal_images_dir();
    let legacy = data_dir_journal_images();
    if same_dir(&dest, &legacy) || !legacy.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(&legacy) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(dir = %legacy.display(), error = %e, "cannot read legacy journal image dir");
            return;
        }
    };
    // Only the `<uuidv4>.<ext>` names we mint — anything else in the
    // directory belongs to someone other than us.
    let names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .filter(|n| is_valid_image_name(n))
                .map(str::to_owned)
        })
        .collect();
    let mut moved = 0usize;
    if !names.is_empty() {
        if let Err(e) = std::fs::create_dir_all(&dest) {
            tracing::warn!(dir = %dest.display(), error = %e, "cannot create journal image dir");
            return;
        }
        for name in names {
            let to = dest.join(&name);
            if to.exists() {
                continue;
            }
            match move_across_volumes(&legacy.join(&name), &to) {
                Ok(()) => moved += 1,
                Err(e) => tracing::warn!(name, error = %e, "failed to relocate journal image"),
            }
        }
    }

    if moved > 0 {
        tracing::info!(
            moved,
            from = %legacy.display(),
            to = %dest.display(),
            "relocated journal images out of the data dir default"
        );
    }
    // Succeeds only once the directory is empty, which is exactly when we
    // want it gone so later boots stop scanning it.
    let _ = std::fs::remove_dir(&legacy);
}

/// Whether two paths name the same directory, resolving symlinks and
/// `.`/`..` when both exist so `./data/journal-images` and an absolute spelling
/// of it aren't treated as two places.
fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Move one file that may be crossing a volume boundary: `rename` first
/// (atomic, and all it ever is on one filesystem), else copy into a `.part`
/// sibling and rename that into place, so a partial copy is never visible
/// under a real serving name. The source is unlinked only once the
/// destination is complete.
fn move_across_volumes(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let part = to.with_extension("part");
    if let Err(e) = std::fs::copy(from, &part) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, to)?;
    std::fs::remove_file(from)
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
