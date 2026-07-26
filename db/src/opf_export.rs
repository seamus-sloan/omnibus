//! OPF 2.0 sidecar export (F5.8) — bakes a book's effective metadata (scanned
//! values merged with `metadata_overrides`) into a portable `metadata.opf`
//! next to its EPUB, so corrections travel to other tools (Calibre,
//! Audiobookshelf). One-shot and write-only: the DB stays the source of
//! truth, and the scanner ignores sidecar `metadata.opf` on reindex.

use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use omnibus_shared::EbookMetadata;

/// Result of a successful export.
#[derive(Debug)]
pub struct OpfExport {
    /// Scan-root-relative path of the written `metadata.opf`, as stored in
    /// the DB (`book_file_relative_dir`) — callers must not echo the
    /// server's absolute directory layout back to a `can_edit`-but-not-
    /// admin client.
    pub path: PathBuf,
    /// Whether a pre-existing `metadata.opf` was renamed to
    /// `metadata.opf.bak` before writing.
    pub backed_up: bool,
}

/// Errors from the OPF export path.
///
/// `BookNotFound` / `NoEpubFile` are the two states a caller branches on
/// (404 / 409); DB lookups surface as their owning module's typed `Books`
/// error and filesystem failures as `Io`.
#[derive(Debug, thiserror::Error)]
pub enum OpfExportError {
    #[error("book {0} not found")]
    BookNotFound(i64),
    #[error("book {0} has no EPUB file — OPF export writes next to the EPUB")]
    NoEpubFile(i64),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
}

/// Export `book_id`'s merged metadata to `{book_dir}/metadata.opf`.
///
/// Snapshots the *effective* metadata exactly as `get_book` returns it —
/// overrides already applied and gated by the owning scan root's
/// metadata-source precedence, so an admin who ranks embedded tags above
/// overrides intentionally exports the scanned values (this is deliberate,
/// not a bug). The sidecar lands next to the lowest-ordinal EPUB (the same
/// "hero file" the Send-to-Kindle export uses); an audiobook-only book has
/// no EPUB and errors with `NoEpubFile`.
///
/// Any existing `metadata.opf` is renamed to `metadata.opf.bak` first
/// (single-level backup — a second export overwrites the prior `.bak`). A
/// cover override is copied alongside as `cover.jpg`.
pub async fn export_opf(pool: &SqlitePool, book_id: i64) -> Result<OpfExport, OpfExportError> {
    let book = crate::books::get_book(pool, book_id)
        .await?
        .ok_or(OpfExportError::BookNotFound(book_id))?;

    let epub_path = crate::books::book_file_path(pool, book_id, "EPUB")
        .await?
        .ok_or(OpfExportError::NoEpubFile(book_id))?;
    let book_dir = epub_path
        .parent()
        .ok_or(OpfExportError::NoEpubFile(book_id))?;
    // Scan-root-relative directory (as stored in the DB) for the *returned*
    // path — never expose the server's absolute filesystem layout to a
    // can_edit-but-not-admin client.
    let relative_dir = crate::books::book_file_relative_dir(pool, book_id, "EPUB")
        .await?
        .unwrap_or_default();

    let xml = render_opf(&book);
    let backed_up = write_sidecar(book_dir, &xml).await?;

    if book.has_cover_override {
        if let Some(uuid) = book.unique_identifier.clone() {
            copy_override_cover(book_dir, uuid).await?;
        }
    }

    Ok(OpfExport {
        path: relative_dir.join("metadata.opf"),
        backed_up,
    })
}

/// Write `xml` to `{book_dir}/metadata.opf`, backing up any existing sidecar
/// to `metadata.opf.bak` first. Returns whether a backup was made.
///
/// The new content is staged to a hidden temp file *before* the original is
/// disturbed, so a failed write can never leave the book without a
/// `metadata.opf`. On a promotion failure the backup is rolled back into
/// place. A concurrent reader never sees a torn document (the final step is a
/// single atomic rename).
async fn write_sidecar(book_dir: &Path, xml: &str) -> Result<bool, std::io::Error> {
    write_sidecar_inner(book_dir, xml, false).await
}

/// Test-only seam: identical to [`write_sidecar`] but forces the final
/// promotion rename to fail, so a test can assert the `.bak` rollback
/// actually restores the pre-existing sidecar. A real filesystem fault can't
/// be injected between the two renames without racing, hence the flag.
#[cfg(test)]
async fn write_sidecar_forcing_promote_failure(
    book_dir: &Path,
    xml: &str,
) -> Result<bool, std::io::Error> {
    write_sidecar_inner(book_dir, xml, true).await
}

async fn write_sidecar_inner(
    book_dir: &Path,
    xml: &str,
    force_promote_failure: bool,
) -> Result<bool, std::io::Error> {
    let opf = book_dir.join("metadata.opf");
    let bak = book_dir.join("metadata.opf.bak");
    let tmp = book_dir.join(".metadata.opf.tmp");

    tokio::fs::write(&tmp, xml.as_bytes()).await?;

    let backed_up = match tokio::fs::rename(&opf, &bak).await {
        Ok(()) => true,
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };

    let promoted = if force_promote_failure {
        Err(std::io::Error::other(
            "forced promotion failure (test seam)",
        ))
    } else {
        tokio::fs::rename(&tmp, &opf).await
    };

    if let Err(e) = promoted {
        // Promotion failed after the backup — restore the original so the
        // book isn't left without a sidecar.
        if backed_up {
            let _ = tokio::fs::rename(&bak, &opf).await;
        }
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(backed_up)
}

/// Copy the override cover for `uuid` into `{book_dir}/cover.jpg`, re-encoding
/// to JPEG when the source isn't already JPEG. A missing or undecodable
/// override (e.g. SVG) is logged and skipped — it never fails the export.
async fn copy_override_cover(book_dir: &Path, uuid: String) -> Result<(), OpfExportError> {
    let jpeg = tokio::task::spawn_blocking(move || encode_override_cover_jpeg(&uuid))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if let Some(bytes) = jpeg {
        tokio::fs::write(book_dir.join("cover.jpg"), &bytes).await?;
    }
    Ok(())
}

/// Load the override cover and return JPEG bytes, or `None` when there is no
/// override file or it can't be decoded. Synchronous (`std::fs` + image
/// codec) — runs on the blocking pool.
fn encode_override_cover_jpeg(uuid: &str) -> Option<Vec<u8>> {
    let (mime, bytes) = crate::covers::find_override_cover_file(uuid)?;
    if mime == "image/jpeg" {
        return Some(bytes);
    }
    match image::load_from_memory(&bytes) {
        Ok(img) => {
            let mut out = Vec::new();
            let rgb = img.to_rgb8();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut out);
            match encoder.encode_image(&rgb) {
                Ok(()) => Some(out),
                Err(e) => {
                    tracing::warn!(target: "omnibus::opf_export", uuid, %e, "cover re-encode failed; skipping");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!(target: "omnibus::opf_export", uuid, %e, "override cover undecodable; skipping");
            None
        }
    }
}

/// Render `book` as an OPF 2.0 `<package>` document (metadata only, no
/// manifest/spine — the Calibre sidecar convention). Pure and deterministic:
/// fields emit in a fixed order so the output is byte-stable for tests.
///
/// Contributors were merged into `creators` at parse time and the schema
/// can't tell them apart, so every creator re-exports as `<dc:creator>`.
pub fn render_opf(book: &EbookMetadata) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uuid_id\" version=\"2.0\">\n",
    );
    out.push_str(
        "  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n",
    );

    push_identifiers(&mut out, book);
    push_title_and_creators(&mut out, book);

    // Description is ammonia-sanitized HTML; escape it once as a text node —
    // a consumer that unescapes gets the markup back (Calibre-identical).
    push_dc(&mut out, "description", book.description.as_deref());
    push_dc(&mut out, "publisher", book.publisher.as_deref());
    push_dc(&mut out, "date", book.published.as_deref());
    push_dc(&mut out, "language", book.language.as_deref());

    push_subjects(&mut out, book);
    push_series_metas(&mut out, book);

    out.push_str("  </metadata>\n");
    out.push_str("</package>\n");
    out
}

/// Emit the package's `dc:identifier` elements: the durable-uuid identifier
/// anchored by `unique-identifier="uuid_id"`, every scanned identifier, and
/// an ISBN override not already among them.
fn push_identifiers(out: &mut String, book: &EbookMetadata) {
    // Package unique identifier (the durable books.uuid). Always emitted so
    // the `unique-identifier` attribute reference resolves — OPF 2.0
    // requires it. `unique_identifier` is always present via `get_book`; the
    // filename fallback keeps a directly-built `EbookMetadata` (uuid `None`)
    // valid rather than dangling the reference.
    let package_id = book
        .unique_identifier
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&book.filename);
    let _ = writeln!(
        out,
        "    <dc:identifier opf:scheme=\"uuid\" id=\"uuid_id\">{}</dc:identifier>",
        xml_escape(package_id)
    );

    // Scanned identifiers (projection order is deterministic).
    for ident in &book.identifiers {
        match ident.scheme.as_deref() {
            Some(scheme) => {
                let _ = writeln!(
                    out,
                    "    <dc:identifier opf:scheme=\"{}\">{}</dc:identifier>",
                    xml_escape(scheme),
                    xml_escape(&ident.value)
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "    <dc:identifier>{}</dc:identifier>",
                    xml_escape(&ident.value)
                );
            }
        }
    }

    // ISBN override that exists only in metadata_overrides (not already among
    // the scanned identifiers). Raw string equality — a hyphenated scanned
    // ISBN duplicating an unhyphenated override yields two lines (harmless).
    if let Some(isbn) = book.isbn13.as_deref().filter(|s| !s.is_empty()) {
        if !book.identifiers.iter().any(|i| i.value == isbn) {
            let _ = writeln!(
                out,
                "    <dc:identifier opf:scheme=\"ISBN\">{}</dc:identifier>",
                xml_escape(isbn)
            );
        }
    }
}

/// Emit `dc:title` (mandatory in OPF 2.0, falling back to the filename the
/// way the UI does for untitled books) and one `dc:creator` per contributor.
/// Contributors were merged into `creators` at parse time and the schema
/// can't tell them apart, so every creator re-exports as `<dc:creator>`.
fn push_title_and_creators(out: &mut String, book: &EbookMetadata) {
    let title = book
        .title
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&book.filename);
    let _ = writeln!(out, "    <dc:title>{}</dc:title>", xml_escape(title));

    for creator in &book.creators {
        let mut attrs = String::new();
        if let Some(role) = creator.role.as_deref().filter(|s| !s.is_empty()) {
            let _ = write!(attrs, " opf:role=\"{}\"", xml_escape(role));
        }
        if let Some(file_as) = creator.file_as.as_deref().filter(|s| !s.is_empty()) {
            let _ = write!(attrs, " opf:file-as=\"{}\"", xml_escape(file_as));
        }
        let _ = writeln!(
            out,
            "    <dc:creator{}>{}</dc:creator>",
            attrs,
            xml_escape(&creator.name)
        );
    }
}

/// Emit one `dc:subject` per subject tag.
fn push_subjects(out: &mut String, book: &EbookMetadata) {
    for subject in &book.subjects {
        let _ = writeln!(out, "    <dc:subject>{}</dc:subject>", xml_escape(subject));
    }
}

/// Emit the Calibre EPUB2 series `<meta>` pair — the same keys the parser
/// falls back to, so an export→rescan round-trips.
fn push_series_metas(out: &mut String, book: &EbookMetadata) {
    if let Some(series) = book.series.as_deref().filter(|s| !s.is_empty()) {
        let _ = writeln!(
            out,
            "    <meta name=\"calibre:series\" content=\"{}\"/>",
            xml_escape(series)
        );
    }
    if let Some(index) = book.series_index.as_deref().filter(|s| !s.is_empty()) {
        let _ = writeln!(
            out,
            "    <meta name=\"calibre:series_index\" content=\"{}\"/>",
            xml_escape(index)
        );
    }
}

/// Emit `<dc:{tag}>{value}</dc:{tag}>` when `value` is present and non-empty.
fn push_dc(out: &mut String, tag: &str, value: Option<&str>) {
    if let Some(v) = value.filter(|s| !s.is_empty()) {
        let _ = writeln!(out, "    <dc:{tag}>{}</dc:{tag}>", xml_escape(v));
    }
}

/// Escape the five XML metacharacters and strip XML-illegal control
/// characters. Used for both text nodes and attribute values —
/// over-escaping `"`/`'` in a text node is still valid XML.
pub(crate) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|c| !is_xml_illegal_control_char(*c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// True for control characters that are valid UTF-8 but illegal in XML 1.0
/// text (everything below 0x20 except tab, LF, and CR).
fn is_xml_illegal_control_char(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}'..='\u{C}' | '\u{E}'..='\u{1F}')
}

#[cfg(test)]
mod tests;
