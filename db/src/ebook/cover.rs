//! Cover-bytes resolver for the ebook scanner.
//!
//! Prefers a sidecar image next to the EPUB and falls back to the embedded
//! cover. With [`super::ScanOptions::materialize_sidecars`] set, freshly
//! extracted embedded covers are written back as a sidecar so the next
//! scan skips the zip. Called from [`super::parse`].

use std::path::{Path, PathBuf};

use epub::doc::EpubDoc;

use crate::library_layout;

use super::ScanOptions;

/// Sidecar-first cover resolution.
///
/// 1. If `<path>` has a sidecar (per-stem first, folder-level fallback),
///    read its bytes and return them.
/// 2. Otherwise, ask the EPUB for its embedded cover.
/// 3. If `opts.materialize_sidecars` is set and the embedded cover came back
///    successfully, write it to a `<basename>.{jpg|png}` sidecar so the next
///    scan hits the sidecar directly. Failures are non-fatal.
///
/// Returns the cover bytes used for *this* scan (the in-memory copy, even
/// when materialization wrote them to disk — this avoids a round-trip read).
pub(crate) fn resolve_cover<R: std::io::Read + std::io::Seek>(
    path: &Path,
    doc: &mut EpubDoc<R>,
    opts: &ScanOptions,
) -> Option<(String, Vec<u8>)> {
    resolve_cover_with(path, opts, || {
        doc.get_cover().map(|(bytes, mime)| {
            let mime = if mime.is_empty() {
                "image/jpeg".to_string()
            } else {
                mime
            };
            (mime, bytes)
        })
    })
}

/// Format-agnostic core of [`resolve_cover`]: the sidecar-first lookup and
/// materialization around a caller-supplied embedded-cover extractor.
/// `embedded` is only invoked when no valid sidecar exists, so a sidecar
/// hit never pays the archive read. Shared with [`crate::comic`], whose
/// "embedded cover" is the archive's first page.
pub(crate) fn resolve_cover_with(
    path: &Path,
    opts: &ScanOptions,
    embedded: impl FnOnce() -> Option<(String, Vec<u8>)>,
) -> Option<(String, Vec<u8>)> {
    let mut corrupt_sidecar: Option<PathBuf> = None;
    if let Some(sidecar) = library_layout::sidecar_cover_for(path) {
        if let Some(bytes) = read_sidecar(&sidecar) {
            return Some(bytes);
        }
        // Sidecar lookup found a file but reading it failed — fall through
        // to the embedded path. Pass the broken path to materialize_sidecar,
        // which self-heals the cache only when the embedded cover materializes
        // to that exact path (same extension); otherwise the corrupt file is
        // left untouched and we fall back to the embedded cover for this scan.
        corrupt_sidecar = Some(sidecar);
    }

    let embedded = embedded();

    if opts.materialize_sidecars {
        if let Some((mime, bytes)) = embedded.as_ref() {
            materialize_sidecar(path, mime, bytes, corrupt_sidecar.as_deref());
        }
    }

    embedded
}

fn read_sidecar(path: &Path) -> Option<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    // A zero-length file is no better than a read error — surfacing it as
    // "the cover" would just blank the row in the UI. Treat as corrupt so
    // the materialize path can repair it next pass.
    if bytes.is_empty() {
        return None;
    }
    let mime = mime_for_extension(path).to_string();
    Some((mime, bytes))
}

fn mime_for_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        // jpg / jpeg / anything else falls back to JPEG. Embedded EPUB covers
        // are overwhelmingly JPEG, and a wrong-but-close mime is better than
        // none for the cover endpoint.
        _ => "image/jpeg",
    }
}

/// Best-effort write of `<basename>.{jpg|png}` next to the epub so future
/// scans skip the zip. Fails silently — this is a cache, not a contract.
///
/// `corrupt_sidecar`, when set, is a sidecar path the caller already
/// confirmed is unreadable. We allow overwriting *exactly* that file so a
/// corrupt cache entry self-heals on the next scan instead of forcing every
/// future scan to re-open the zip. Anything else under `target.exists()`
/// (a valid file, a different filename) we leave alone.
pub(crate) fn materialize_sidecar(
    epub_path: &Path,
    mime: &str,
    bytes: &[u8],
    corrupt_sidecar: Option<&Path>,
) {
    let Some(parent) = epub_path.parent() else {
        return;
    };
    let Some(stem) = epub_path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    let ext = if mime.eq_ignore_ascii_case("image/png") {
        "png"
    } else {
        "jpg"
    };
    let target = parent.join(format!("{stem}.{ext}"));
    if target.exists() {
        let is_known_corrupt = corrupt_sidecar.is_some_and(|p| p == target.as_path());
        if !is_known_corrupt {
            // A valid file we don't own (race or user-dropped sidecar). Don't
            // clobber.
            return;
        }
        // Fall through and overwrite — std::fs::write truncates the existing
        // file, repairing the cache.
        tracing::warn!(
            path = %target.display(),
            "repairing unreadable cover sidecar"
        );
    }
    if let Err(e) = std::fs::write(&target, bytes) {
        tracing::warn!(
            error = %e,
            path = %target.display(),
            "could not materialize cover sidecar; falling back to embedded for this scan"
        );
    }
}

#[cfg(test)]
mod tests;
