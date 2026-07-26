//! Filesystem half of deletion: unlink the deleted files and their per-stem
//! sidecar covers, prune the directories they emptied, and — on a total
//! delete — drop the book's regenerable caches and journal images. Every step
//! is best-effort and logged rather than propagated (mirroring
//! `missing_files::cleanup_victim_covers`): the DB has already committed.

use std::path::{Path, PathBuf};

/// Everything one delete needs to clean off disk.
pub(super) struct Cleanup {
    pub paths: Vec<PathBuf>,
    pub library_roots: Vec<String>,
    /// Present only when the `books` row went too.
    pub book: Option<BookCleanup>,
}

/// The book-scoped extras a total delete also removes.
pub(super) struct BookCleanup {
    pub book_id: i64,
    pub uuid: String,
    pub journal_images: Vec<String>,
}

/// Run the cleanup off the runtime. A `JoinError` is logged and swallowed for
/// the same reason the individual steps are.
pub(super) async fn cleanup(job: Cleanup) {
    if let Err(join_err) = tokio::task::spawn_blocking(move || cleanup_blocking(job)).await {
        tracing::error!("delete_book_files: fs cleanup spawn_blocking failed: {join_err}");
    }
}

/// Sync body of [`cleanup`].
fn cleanup_blocking(job: Cleanup) {
    let roots: Vec<PathBuf> = job.library_roots.iter().map(PathBuf::from).collect();
    for path in &job.paths {
        remove_file(path);
        // The scanner writes a per-stem sidecar cover next to the file; left
        // behind it would keep the folder non-empty and litter the library.
        if let Some(cover) = crate::library_layout::per_stem_sidecar_cover(path) {
            remove_file(&cover);
        }
        prune_empty_dirs(path.parent(), &roots);
    }
    let Some(book) = job.book else {
        return;
    };
    crate::metadata_overrides::delete_override_cover(&book.uuid);
    crate::covers::delete_cover_files_for(std::slice::from_ref(&book.uuid));
    remove_thumbnails(book.book_id);
    remove_dir(&crate::hls::hls_dir().join(book.book_id.to_string()));
    remove_file(&crate::kepub::kepub_path(book.book_id));
    crate::epub_rewrite::invalidate_export_epub_cache(book.book_id);
    for name in &book.journal_images {
        crate::journal_images::delete_journal_image(name);
    }
}

/// Unlink one file. A missing file is a no-op — the delete is idempotent and
/// the DB row is already gone either way.
fn remove_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = ?path, error = %e, "delete: failed to remove file");
        }
    }
}

/// Recursively remove a directory tree, tolerating its absence.
fn remove_dir(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = ?path, error = %e, "delete: failed to remove directory");
        }
    }
}

/// Remove the WebP thumbnails cached for `book_id`, all three sizes.
fn remove_thumbnails(book_id: i64) {
    for size in [
        crate::thumbs::ThumbSize::Sm,
        crate::thumbs::ThumbSize::Md,
        crate::thumbs::ThumbSize::Lg,
    ] {
        remove_file(&crate::thumbs::thumb_path_for(book_id, size));
    }
}

/// Walk up from `start`, removing each directory that the delete left empty,
/// and stop at the first non-empty one. Never touches a scan root itself or
/// anything outside one: the canonical layout nests files two deep
/// (`<root>/<author>/<title>/<title>.epub`), so a deleted book would otherwise
/// leave a pair of empty folders behind.
fn prune_empty_dirs(start: Option<&Path>, roots: &[PathBuf]) {
    let mut current = start.map(Path::to_path_buf);
    while let Some(dir) = current {
        if roots.iter().any(|root| root == &dir) || !is_within_a_root(&dir, roots) {
            return;
        }
        // `remove_dir` on a non-empty directory fails, which is exactly the
        // stop condition — no separate emptiness check to race against.
        if std::fs::remove_dir(&dir).is_err() {
            return;
        }
        current = dir.parent().map(Path::to_path_buf);
    }
}

/// Whether `dir` sits strictly inside one of the configured scan roots. Guards
/// the prune walk from climbing past a library into the wider filesystem.
fn is_within_a_root(dir: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| dir.starts_with(root) && dir != root.as_path())
}
