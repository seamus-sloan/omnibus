//! "Add your own books" upload handlers (web-facing REST).
//!
//! Two-step ebook ingest: `inspect` parses an uploaded EPUB and returns its
//! embedded metadata for an editable confirm step; the commit endpoint files
//! the file into the canonical library folder, reindexes so the indexer owns
//! the insert, then layers the user's edits as metadata overrides. Audiobook
//! endpoints are stubbed (501) until audio ingest lands.

use std::path::PathBuf;

use axum::{
    extract::{multipart::Field, Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    self as db, library_layout,
    worker::{Task, TaskOutcome},
};
use omnibus_shared::{
    detect_ebook_format, Contributor, MetadataOverrides, UploadCommitResult, UploadInspection,
};
use tokio::io::AsyncWriteExt as _;

use super::AppState;
use crate::auth::AuthUser;

/// Default upload size cap (1 GiB) when `OMNIBUS_MAX_UPLOAD_BYTES` is unset or
/// unparseable. Generous so it survives the future large-audiobook case.
pub const DEFAULT_MAX_UPLOAD_BYTES: usize = 1024 * 1024 * 1024;

/// Resolve the configured upload size cap from `OMNIBUS_MAX_UPLOAD_BYTES`,
/// falling back to [`DEFAULT_MAX_UPLOAD_BYTES`]. Read at router-build time for
/// the body limit and again per-request as a defense-in-depth backstop.
pub fn max_upload_bytes() -> usize {
    std::env::var("OMNIBUS_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_BYTES)
}

/// Predictable upload failures. Mapped to HTTP at the boundary via
/// [`IntoResponse`] so handlers stay a linear `?` flow. A small enum (not a
/// boxed `Response`) keeps the `Result` cheap to pass around.
#[derive(Debug)]
pub(super) enum UploadError {
    /// Caller lacks `can_upload`/admin → 403.
    Forbidden,
    /// No ebook library path configured → 400.
    NotConfigured,
    /// File field absent from the multipart body → 400.
    MissingFile,
    /// Title/author missing, so the file can't be placed → 400.
    MissingMetadata,
    /// File isn't a recognizable EPUB → 415.
    UnsupportedFormat,
    /// File can't be opened/parsed as an EPUB → 415 (carries the reason).
    BadEpub(String),
    /// File exceeds the configured byte cap → 413.
    TooLarge(usize),
    /// Override validation failed (a field too long) → 400.
    Validation(String),
    /// Unexpected internal failure → 500 (logged; detail not leaked to the wire).
    Internal {
        context: &'static str,
        detail: String,
    },
}

impl UploadError {
    fn internal(context: &'static str, e: impl std::fmt::Display) -> Self {
        UploadError::Internal {
            context,
            detail: e.to_string(),
        }
    }
}

impl IntoResponse for UploadError {
    fn into_response(self) -> Response {
        match self {
            UploadError::Forbidden => {
                (StatusCode::FORBIDDEN, "upload permission required").into_response()
            }
            UploadError::NotConfigured => (
                StatusCode::BAD_REQUEST,
                "Configure an ebook library path in Settings first",
            )
                .into_response(),
            UploadError::MissingFile => {
                (StatusCode::BAD_REQUEST, "missing 'file' field").into_response()
            }
            UploadError::MissingMetadata => (
                StatusCode::BAD_REQUEST,
                "title and author are required to file the book",
            )
                .into_response(),
            UploadError::UnsupportedFormat => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "file must be a valid EPUB",
            )
                .into_response(),
            UploadError::BadEpub(msg) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, msg).into_response(),
            UploadError::TooLarge(cap) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("file exceeds the {cap}-byte upload limit"),
            )
                .into_response(),
            UploadError::Validation(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            UploadError::Internal { context, detail } => {
                tracing::error!(error = %detail, context = context, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

/// 403 unless the user may upload (`can_upload` or admin). Mirrors the
/// `can_edit` gate in [`super::overrides`].
fn require_upload(user: &AuthUser) -> Result<(), UploadError> {
    if user.is_admin || user.can_upload {
        Ok(())
    } else {
        Err(UploadError::Forbidden)
    }
}

/// Magic-byte + size gate for a fully-buffered upload. Kept as a unit-testable
/// utility; the streaming path enforces both invariants incrementally inside
/// [`stream_upload_to_tempfile`].
#[allow(dead_code)]
fn validate_file_bytes(bytes: &[u8], cap: usize) -> Result<(), UploadError> {
    if bytes.len() > cap {
        return Err(UploadError::TooLarge(cap));
    }
    if detect_ebook_format(bytes).is_none() {
        return Err(UploadError::UnsupportedFormat);
    }
    Ok(())
}

/// Trim and drop empty so blank form fields read as "no value".
fn norm(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

// --- Streaming helper -------------------------------------------------------

/// Stream a multipart `file` field to a newly-created tempfile, enforcing the
/// byte cap incrementally and validating EPUB magic bytes on the first chunk.
/// The payload is never fully buffered in RAM — only one chunk is held at a
/// time while it is written to disk.
async fn stream_upload_to_tempfile(
    mut field: Field<'_>,
    cap: usize,
) -> Result<tempfile::NamedTempFile, UploadError> {
    let tmp = tempfile::Builder::new()
        .suffix(".epub")
        .tempfile()
        .map_err(|e| UploadError::internal("create upload tempfile", e))?;
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .open(tmp.path())
        .await
        .map_err(|e| UploadError::internal("open upload tempfile", e))?;

    let mut total = 0usize;
    let mut format_validated = false;

    loop {
        let chunk = field
            .chunk()
            .await
            .map_err(|e| UploadError::internal("read upload chunk", e))?;
        let Some(chunk) = chunk else { break };
        if !format_validated {
            if detect_ebook_format(&chunk).is_none() {
                return Err(UploadError::UnsupportedFormat);
            }
            format_validated = true;
        }
        total += chunk.len();
        if total > cap {
            return Err(UploadError::TooLarge(cap));
        }
        f.write_all(&chunk)
            .await
            .map_err(|e| UploadError::internal("write upload chunk", e))?;
    }

    if !format_validated {
        return Err(UploadError::MissingFile);
    }
    Ok(tmp)
}

// --- Inspect ---------------------------------------------------------------

/// Parse an uploaded EPUB and return its embedded metadata for the editable
/// confirm step. Stateless: the field is streamed to a tempfile, parsed, then
/// discarded.
pub(super) async fn post_inspect_ebook(
    user: AuthUser,
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, UploadError> {
    require_upload(&user)?;
    let field = loop {
        match multipart.next_field().await {
            Ok(Some(f)) if f.name().unwrap_or("") == "file" => break f,
            Ok(Some(_)) => continue,
            Ok(None) => return Err(UploadError::MissingFile),
            Err(e) => return Err(UploadError::internal("parse multipart", e)),
        }
    };
    let tmp = stream_upload_to_tempfile(field, max_upload_bytes()).await?;
    // Parsing opens a zip + reads the OPF — run it off the async runtime.
    let inspection = tokio::task::spawn_blocking(move || inspect_ebook_tempfile(&tmp))
        .await
        .map_err(|e| UploadError::internal("spawn_blocking(inspect ebook)", e))??;
    Ok(Json(inspection).into_response())
}

/// Parse the already-written `tmp` file as an EPUB and project the result into
/// an [`UploadInspection`]. Parse failures map to 415; staging IO failures to
/// 500.
fn inspect_ebook_tempfile(tmp: &tempfile::NamedTempFile) -> Result<UploadInspection, UploadError> {
    let size_bytes = std::fs::metadata(tmp.path())
        .map_err(|e| UploadError::internal("stat upload tempfile", e))?
        .len() as i64;
    let targets = vec![db::ebook::ParseTarget {
        filename: "upload.epub".to_string(),
        absolute: tmp.path().to_path_buf(),
        mtime_epoch: 0,
        size_bytes,
    }];
    let mut parsed = db::ebook::parse_ebook_targets(targets, db::ebook::ScanOptions::default());
    let book = parsed
        .pop()
        .ok_or_else(|| UploadError::BadEpub("could not parse EPUB".to_string()))?;
    if let Some(err) = book.metadata.error {
        return Err(UploadError::BadEpub(format!("could not parse EPUB: {err}")));
    }
    Ok(UploadInspection {
        title: book.metadata.title,
        author: book.metadata.creators.first().map(|c| c.name.clone()),
        series: book.metadata.series,
        series_index: book.metadata.series_index,
        language: book.metadata.language,
        has_cover: book.cover.is_some(),
        ext: "epub".to_string(),
    })
}

// --- Commit ----------------------------------------------------------------

/// The user's (possibly edited) metadata parsed from the commit multipart body,
/// plus a tempfile holding the already-streamed EPUB bytes.
#[derive(Default)]
struct CommitForm {
    tmp_file: Option<tempfile::NamedTempFile>,
    title: Option<String>,
    author: Option<String>,
    series: Option<String>,
    series_index: Option<String>,
}

/// File the uploaded EPUB into the canonical library folder using the user's
/// confirmed title/author, reindex so the indexer inserts the book, then layer
/// any edits as metadata overrides. Returns 201 with the new book's uuid.
pub(super) async fn post_upload_ebook(
    user: AuthUser,
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, UploadError> {
    require_upload(&user)?;
    let mut form = parse_commit_multipart(multipart, max_upload_bytes()).await?;
    let tmp = form.tmp_file.take().ok_or(UploadError::MissingFile)?;
    let (Some(title), Some(author)) = (norm(&form.title), norm(&form.author)) else {
        return Err(UploadError::MissingMetadata);
    };

    // Library root must be configured before any file can be placed.
    let settings = db::get_settings(&state.pool)
        .await
        .map_err(|e| UploadError::internal("get_settings", e))?;
    let root = settings
        .ebook_library_path
        .filter(|p| !p.is_empty())
        .ok_or(UploadError::NotConfigured)?;

    // Allocate a non-colliding canonical path and copy the tempfile there.
    let root_path = PathBuf::from(&root);
    let dest = library_layout::allocate_canonical_path(&root_path, &author, &title, "epub")
        .map_err(|e| UploadError::internal("allocate_canonical_path", e))?;
    let dest_for_copy = dest.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(parent) = dest_for_copy.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(tmp.path(), &dest_for_copy).map(|_| ())?;
        // tmp drops here, deleting the upload tempfile.
        Ok(())
    })
    .await
    .map_err(|e| UploadError::internal("spawn_blocking(file ebook)", e))?
    .map_err(|e| UploadError::internal("file uploaded ebook", e))?;

    // Reindex so the indexer mints the uuid, extracts the cover, and updates
    // FTS — the single source of truth for inserting books.
    let task_id = state.worker.post(Task::Scan {
        library_path: root.clone(),
    });
    if let TaskOutcome::Err(e) = state.worker.await_completion(task_id).await {
        // Don't strand a file whose scan failed.
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(UploadError::internal("reindex after upload", e));
    }

    // Map the file we just placed back to its row via the durable scan_key.
    // Clean up dest on any failure to avoid orphaning the file on disk.
    let scan_key = dest
        .strip_prefix(&root_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let uuid_result = db::get_book_uuid_by_scan_key(&state.pool, &root, &scan_key)
        .await
        .map_err(|e| UploadError::internal("get_book_uuid_by_scan_key", e))
        .and_then(|opt| {
            opt.ok_or_else(|| {
                UploadError::internal(
                    "resolve uploaded book",
                    "reindex did not surface the uploaded file",
                )
            })
        });
    let uuid = match uuid_result {
        Ok(uuid) => uuid,
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    };

    // Make the displayed metadata match what the user confirmed.
    apply_user_edits(&state, &uuid, &form, user.id).await?;

    Ok((StatusCode::CREATED, Json(UploadCommitResult { uuid })).into_response())
}

/// Collect the user's text fields and stream the file field to a tempfile.
async fn parse_commit_multipart(
    mut multipart: Multipart,
    cap: usize,
) -> Result<CommitForm, UploadError> {
    let mut form = CommitForm::default();
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                match name.as_str() {
                    "file" => {
                        form.tmp_file = Some(stream_upload_to_tempfile(field, cap).await?);
                    }
                    "title" => form.title = field.text().await.ok(),
                    "author" => form.author = field.text().await.ok(),
                    "series" => form.series = field.text().await.ok(),
                    "series_index" => form.series_index = field.text().await.ok(),
                    _ => continue,
                }
            }
            Ok(None) => break,
            Err(e) => return Err(UploadError::internal("parse multipart", e)),
        }
    }
    Ok(form)
}

/// Diff the user's confirmed fields against the indexer's embedded values and
/// persist a metadata override for each field they changed.
async fn apply_user_edits(
    state: &AppState,
    uuid: &str,
    form: &CommitForm,
    user_id: i64,
) -> Result<(), UploadError> {
    let book = db::get_book_by_uuid(&state.pool, uuid)
        .await
        .map_err(|e| UploadError::internal("get_book_by_uuid", e))?
        .ok_or_else(|| UploadError::internal("get_book_by_uuid after upload", "book vanished"))?;

    let mut overrides = MetadataOverrides::default();
    let mut changed = false;
    if let Some(title) = norm(&form.title) {
        if book.title.as_deref() != Some(title.as_str()) {
            overrides.title = Some(title);
            changed = true;
        }
    }
    if let Some(author) = norm(&form.author) {
        let embedded = book.creators.first().map(|c| c.name.as_str());
        if embedded != Some(author.as_str()) {
            overrides.creators = Some(vec![Contributor {
                name: author,
                role: None,
                file_as: None,
                id: None,
            }]);
            changed = true;
        }
    }
    if let Some(series) = norm(&form.series) {
        if book.series.as_deref() != Some(series.as_str()) {
            overrides.series = Some(series);
            changed = true;
        }
    }
    if let Some(series_index) = norm(&form.series_index) {
        if book.series_index.as_deref() != Some(series_index.as_str()) {
            overrides.series_index = Some(series_index);
            changed = true;
        }
    }

    if !changed {
        return Ok(());
    }
    overrides.validate().map_err(UploadError::Validation)?;
    db::merge_metadata_overrides(&state.pool, uuid, &overrides, user_id)
        .await
        .map_err(|e| UploadError::internal("merge_metadata_overrides", e))?;
    Ok(())
}

// --- Audiobook stubs -------------------------------------------------------

/// Placeholder until audiobook ingest lands. Gated on upload permission so the
/// contract (auth → 501) matches the ebook endpoints.
pub(super) async fn post_inspect_audiobook(user: AuthUser) -> Result<Response, UploadError> {
    require_upload(&user)?;
    Ok(audiobook_coming_soon())
}

/// Placeholder commit endpoint — see [`post_inspect_audiobook`].
pub(super) async fn post_upload_audiobook(user: AuthUser) -> Result<Response, UploadError> {
    require_upload(&user)?;
    Ok(audiobook_coming_soon())
}

fn audiobook_coming_soon() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "audiobook upload is coming soon",
    )
        .into_response()
}

#[cfg(test)]
mod tests;
