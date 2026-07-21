//! Audiobook ingest handlers for the "add your own books" upload flow.
//! Sibling to the EPUB handlers in [`super`], sharing its error type, gate, and
//! size cap. Accepts a single `.m4a`/`.m4b` container or a set of `.mp3` parts
//! filed into one canonical folder, then reindexes so the indexer inserts it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
    detect_audiobook_format, AudiobookInspection, Contributor, MetadataOverrides,
    UploadCommitResult, AUDIOBOOK_MAGIC_LEN,
};
use tokio::io::AsyncWriteExt as _;

use super::{max_upload_bytes, norm, require_upload, UploadError};
use crate::auth::AuthUser;
use crate::backend::AppState;

/// One streamed audiobook file staged in a tempfile, awaiting placement.
struct AudioUpload {
    /// Validated lowercase extension (`m4a` / `m4b` / `mp3`).
    ext: String,
    /// The client-supplied filename (used to name `.mp3` parts on disk).
    filename: String,
    tmp: tempfile::NamedTempFile,
}

/// Whether an upload is a single `.m4a`/`.m4b` container or a set of `.mp3`
/// parts destined for one folder.
#[derive(Clone, Copy, PartialEq)]
enum AudioKind {
    Single,
    Mp3Set,
}

/// The user's confirmed metadata plus the streamed audiobook file(s).
#[derive(Default)]
struct AudiobookCommitForm {
    files: Vec<AudioUpload>,
    title: Option<String>,
    author: Option<String>,
}

/// Map an uploaded filename to a supported audiobook extension, or `None` if
/// its extension isn't one of [`db::audiobook::AUDIOBOOK_EXTENSIONS`].
fn audiobook_ext_of(filename: &str) -> Option<String> {
    let ext = Path::new(filename)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    db::audiobook::AUDIOBOOK_EXTENSIONS
        .contains(&ext.as_str())
        .then_some(ext)
}

/// Magic-byte container family expected for a given extension: `.m4a`/`.m4b`
/// share the MP4 (`mp4`) container; `.mp3` is its own family.
fn audio_family(ext: &str) -> &'static str {
    if ext == "mp3" {
        "mp3"
    } else {
        "mp4"
    }
}

/// Accumulate up to [`AUDIOBOOK_MAGIC_LEN`] leading bytes across chunks, then
/// confirm they match `family`. Returns `Ok(true)` once validated. Unlike the
/// EPUB path this can't fail-fast at 4 bytes: an MP4 `ftyp` box is only visible
/// once 8 bytes have arrived, so a shorter prefix is inconclusive, not invalid.
fn extend_and_validate_audio_magic(
    prefix: &mut Vec<u8>,
    chunk: &[u8],
    family: &str,
) -> Result<bool, UploadError> {
    let needed = AUDIOBOOK_MAGIC_LEN.saturating_sub(prefix.len());
    prefix.extend_from_slice(&chunk[..chunk.len().min(needed)]);
    if prefix.len() < AUDIOBOOK_MAGIC_LEN {
        return Ok(false);
    }
    match detect_audiobook_format(prefix) {
        Some(f) if f == family => Ok(true),
        _ => Err(UploadError::UnsupportedAudioFormat),
    }
}

/// Stream one audiobook `file` field to a tempfile, enforcing the byte cap
/// incrementally and cross-checking the magic bytes against `ext`'s container
/// family (so an `.mp3` renamed to `.m4b` is rejected before it reaches disk).
async fn stream_audio_to_tempfile(
    mut field: Field<'_>,
    cap: usize,
    ext: &str,
) -> Result<tempfile::NamedTempFile, UploadError> {
    let family = audio_family(ext);
    let tmp = tempfile::Builder::new()
        .suffix(&format!(".{ext}"))
        .tempfile()
        .map_err(|e| UploadError::internal("create audio tempfile", e))?;
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .open(tmp.path())
        .await
        .map_err(|e| UploadError::internal("open audio tempfile", e))?;

    let mut total = 0usize;
    let mut validated = false;
    let mut prefix = Vec::with_capacity(AUDIOBOOK_MAGIC_LEN);

    loop {
        let chunk = field
            .chunk()
            .await
            .map_err(|e| UploadError::internal("read upload chunk", e))?;
        let Some(chunk) = chunk else { break };
        if !validated {
            validated = extend_and_validate_audio_magic(&mut prefix, &chunk, family)?;
        }
        total += chunk.len();
        if total > cap {
            return Err(UploadError::TooLarge(cap));
        }
        f.write_all(&chunk)
            .await
            .map_err(|e| UploadError::internal("write upload chunk", e))?;
    }

    // Flush + fsync before the path is handed to a separate reader (lofty in a
    // `spawn_blocking`, or `std::fs::copy` on commit). Without this the inspect
    // path can reopen the tempfile before the streamed writes are durably
    // visible and lofty reads an empty/partial file → a spurious 415. Surfaced
    // only under CI's Linux filesystem timing; the commit path's extra awaits
    // masked it.
    f.flush()
        .await
        .map_err(|e| UploadError::internal("flush audio tempfile", e))?;
    f.sync_all()
        .await
        .map_err(|e| UploadError::internal("sync audio tempfile", e))?;
    drop(f);

    if prefix.is_empty() {
        return Err(UploadError::MissingFile);
    }
    if !validated {
        return Err(UploadError::UnsupportedAudioFormat);
    }
    Ok(tmp)
}

/// Collect every `file` field (streamed to tempfiles) plus the `title`/`author`
/// text fields from the multipart body. Shared by inspect (which ignores the
/// text fields) and commit.
async fn parse_audiobook_multipart(
    mut multipart: Multipart,
    cap: usize,
) -> Result<AudiobookCommitForm, UploadError> {
    let mut form = AudiobookCommitForm::default();
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                match name.as_str() {
                    "file" => {
                        let filename = field.file_name().unwrap_or("").to_string();
                        let ext = audiobook_ext_of(&filename)
                            .ok_or(UploadError::UnsupportedAudioFormat)?;
                        let tmp = stream_audio_to_tempfile(field, cap, &ext).await?;
                        form.files.push(AudioUpload { ext, filename, tmp });
                    }
                    "title" => form.title = field.text().await.ok(),
                    "author" => form.author = field.text().await.ok(),
                    _ => continue,
                }
            }
            Ok(None) => break,
            Err(e) => return Err(UploadError::internal("parse multipart", e)),
        }
    }
    Ok(form)
}

/// Classify the uploaded file set: a lone `.m4a`/`.m4b` is [`AudioKind::Single`],
/// one-or-more `.mp3` is [`AudioKind::Mp3Set`]. Multiple single-file containers
/// or a mix of families are rejected — each `.m4a`/`.m4b` is its own book.
fn classify_audio_set(files: &[AudioUpload]) -> Result<AudioKind, UploadError> {
    if files.is_empty() {
        return Err(UploadError::MissingFile);
    }
    if files.iter().all(|u| u.ext == "mp3") {
        return Ok(AudioKind::Mp3Set);
    }
    if files.len() == 1 {
        return Ok(AudioKind::Single);
    }
    Err(UploadError::MixedAudioUpload)
}

/// The settled lowercase format for the inspection/commit response: the single
/// container's own extension, or `"mp3"` for a part set.
fn audio_format(kind: AudioKind, files: &[AudioUpload]) -> String {
    match kind {
        AudioKind::Single => files
            .first()
            .map(|u| u.ext.clone())
            .unwrap_or_else(|| "mp3".to_string()),
        AudioKind::Mp3Set => "mp3".to_string(),
    }
}

// --- Inspect ---------------------------------------------------------------

/// Parse the uploaded audiobook file(s) and return book-level metadata for the
/// editable confirm step. Stateless: files are streamed to tempfiles, parsed,
/// then discarded.
pub(in crate::backend) async fn post_inspect_audiobook(
    user: AuthUser,
    State(_state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, UploadError> {
    require_upload(&user)?;
    let form = parse_audiobook_multipart(multipart, max_upload_bytes()).await?;
    let kind = classify_audio_set(&form.files)?;
    let format = audio_format(kind, &form.files);
    let part_count = form.files.len();

    let paths: Vec<PathBuf> = form
        .files
        .iter()
        .map(|u| u.tmp.path().to_path_buf())
        .collect();
    // Tag reads open containers off the async runtime. `form` (and its
    // tempfiles) is held until the blocking task returns so the paths stay valid.
    let inspected =
        tokio::task::spawn_blocking(move || db::audiobook::inspect_audiobook_files(&paths))
            .await
            .map_err(|e| UploadError::internal("spawn_blocking(inspect audiobook)", e))?
            .map_err(|e| UploadError::BadAudio(format!("could not read audiobook: {e}")))?;
    drop(form);

    Ok(Json(AudiobookInspection {
        title: inspected.title,
        author: inspected.author,
        has_cover: inspected.has_cover,
        format,
        part_count,
        duration_seconds: inspected.duration_seconds,
    })
    .into_response())
}

// --- Commit ----------------------------------------------------------------

/// A filed audiobook on disk, plus the durable scan_key the reindex will key
/// it under. Carries enough to undo the placement if the scan later fails.
struct PlacedAudiobook {
    /// The single file (Single) or the parts folder (Mp3Set).
    path: PathBuf,
    scan_key: String,
    is_folder: bool,
}

impl PlacedAudiobook {
    /// Remove the filed audiobook so a failed scan doesn't strand it on disk.
    async fn cleanup(&self) {
        if self.is_folder {
            let _ = tokio::fs::remove_dir_all(&self.path).await;
        } else {
            let _ = tokio::fs::remove_file(&self.path).await;
        }
    }
}

/// File the uploaded audiobook into the canonical audiobook library, reindex so
/// the indexer inserts the book, then layer the confirmed title/author as
/// metadata overrides. Returns 201 with the new book's uuid.
pub(in crate::backend) async fn post_upload_audiobook(
    user: AuthUser,
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, UploadError> {
    require_upload(&user)?;
    let form = parse_audiobook_multipart(multipart, max_upload_bytes()).await?;
    let kind = classify_audio_set(&form.files)?;
    let (Some(title), Some(author)) = (norm(&form.title), norm(&form.author)) else {
        return Err(UploadError::MissingMetadata);
    };

    let settings = db::get_settings(&state.pool)
        .await
        .map_err(|e| UploadError::internal("get_settings", e))?;
    let root = settings
        .audiobook_library_path
        .filter(|p| !p.is_empty())
        .ok_or(UploadError::AudiobookNotConfigured)?;
    let root_path = PathBuf::from(&root);

    let placed = place_audiobook(&root_path, &author, &title, kind, form.files).await?;

    // Reindex so the indexer mints the uuid, extracts the cover + chapters, and
    // writes `book_file_parts` — the single source of truth for inserting books.
    let task_id = state.worker.post(Task::ScanAudiobooks {
        library_path: root.clone(),
    });
    if let TaskOutcome::Err(e) = state.worker.await_completion(task_id).await {
        placed.cleanup().await;
        return Err(UploadError::internal("reindex after audiobook upload", e));
    }

    let uuid = match db::get_book_uuid_by_scan_key(&state.pool, &root, &placed.scan_key).await {
        Ok(Some(uuid)) => uuid,
        Ok(None) => {
            placed.cleanup().await;
            return Err(UploadError::internal(
                "resolve uploaded audiobook",
                "reindex did not surface the uploaded audiobook",
            ));
        }
        Err(e) => {
            placed.cleanup().await;
            return Err(UploadError::internal("get_book_uuid_by_scan_key", e));
        }
    };

    apply_audiobook_edits(&state, &uuid, &title, &author, user.id).await?;

    Ok((StatusCode::CREATED, Json(UploadCommitResult { uuid })).into_response())
}

/// Copy the streamed audiobook file(s) into the canonical library and return
/// the placement (with its durable scan_key). A read-only library maps to
/// [`UploadError::AudiobookLibraryNotWritable`]; other IO failures to 500.
async fn place_audiobook(
    root: &Path,
    author: &str,
    title: &str,
    kind: AudioKind,
    files: Vec<AudioUpload>,
) -> Result<PlacedAudiobook, UploadError> {
    let root = root.to_path_buf();
    let author = author.to_string();
    let title = title.to_string();
    tokio::task::spawn_blocking(move || match kind {
        AudioKind::Single => place_single_blocking(&root, &author, &title, files),
        AudioKind::Mp3Set => place_mp3_folder_blocking(&root, &author, &title, files),
    })
    .await
    .map_err(|e| UploadError::internal("spawn_blocking(file audiobook)", e))?
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(error = %e, "audiobook library is not writable");
            UploadError::AudiobookLibraryNotWritable
        }
        _ => UploadError::internal("file uploaded audiobook", e),
    })
}

/// Place a single `.m4a`/`.m4b` container at its canonical
/// `<root>/<author>/<title>/<title>.<ext>` path.
fn place_single_blocking(
    root: &Path,
    author: &str,
    title: &str,
    files: Vec<AudioUpload>,
) -> std::io::Result<PlacedAudiobook> {
    let file = files
        .first()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file"))?;
    let dest = library_layout::allocate_canonical_path(root, author, title, &file.ext)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(file.tmp.path(), &dest)?;
    let scan_key = relative_scan_key(root, &dest);
    Ok(PlacedAudiobook {
        path: dest,
        scan_key,
        is_folder: false,
    })
}

/// Place a set of `.mp3` parts into one canonical
/// `<root>/<author>/<title>/` folder, sanitizing and de-duplicating each part's
/// filename. The folder's relative path is the group's scan_key (matching the
/// indexer's folder-based mp3 grouping).
fn place_mp3_folder_blocking(
    root: &Path,
    author: &str,
    title: &str,
    files: Vec<AudioUpload>,
) -> std::io::Result<PlacedAudiobook> {
    let folder = library_layout::allocate_canonical_dir(root, author, title)?;
    std::fs::create_dir_all(&folder)?;
    let mut used: HashSet<String> = HashSet::new();
    for file in &files {
        let name = dedupe_part_name(
            library_layout::sanitize_part_filename(&file.filename),
            &mut used,
        );
        std::fs::copy(file.tmp.path(), folder.join(&name))?;
    }
    let scan_key = relative_scan_key(root, &folder);
    Ok(PlacedAudiobook {
        path: folder,
        scan_key,
        is_folder: true,
    })
}

/// Library-relative path of `dest` under `root`, used as the reindex scan_key.
fn relative_scan_key(root: &Path, dest: &Path) -> String {
    dest.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Ensure a part filename is unique within its folder by inserting a numeric
/// suffix (`name.mp3` → `name-2.mp3`) before the extension on collision.
fn dedupe_part_name(name: String, used: &mut HashSet<String>) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let path = Path::new(&name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(&name);
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 2..=9999 {
        let candidate = match ext {
            Some(ext) => format!("{stem}-{n}.{ext}"),
            None => format!("{stem}-{n}"),
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    name
}

/// Diff the user's confirmed title/author against the indexer's embedded values
/// and persist a metadata override for each field they changed.
async fn apply_audiobook_edits(
    state: &AppState,
    uuid: &str,
    title: &str,
    author: &str,
    user_id: i64,
) -> Result<(), UploadError> {
    let book = db::get_book_by_uuid(&state.pool, uuid)
        .await
        .map_err(|e| UploadError::internal("get_book_by_uuid", e))?
        .ok_or_else(|| UploadError::internal("get_book_by_uuid after upload", "book vanished"))?;

    let mut overrides = MetadataOverrides::default();
    let mut changed = false;
    if book.title.as_deref() != Some(title) {
        overrides.title = Some(title.to_string());
        changed = true;
    }
    let embedded = book.creators.first().map(|c| c.name.as_str());
    if embedded != Some(author) {
        overrides.creators = Some(vec![Contributor {
            name: author.to_string(),
            role: None,
            file_as: None,
            id: None,
        }]);
        changed = true;
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
