//! CBZ comic-archive metadata extraction (server-only). Sibling to
//! [`crate::ebook`]'s EPUB parser: opens the zip, lists image pages in
//! natural-sort order, parses `ComicInfo.xml` when present, and produces
//! the same [`IndexedBook`] shape the sync writer consumes. Also the read
//! surface's page source: [`read_page`] hands the page endpoint one
//! decompressed entry at a time.

use std::cmp::Ordering;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use omnibus_shared::{Contributor, EbookMetadata};

use crate::ebook::{extract_accent, resolve_cover_with, IndexedBook, ScanOptions};

/// Image-entry extensions that count as comic pages, compared
/// case-insensitively against each archive entry's extension.
pub const COMIC_PAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

/// `true` when `path` is a CBZ archive by extension — the dispatch test
/// [`crate::ebook::parse_ebook_targets`] uses to route a scan target here
/// instead of the EPUB parser.
pub fn is_comic_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cbz"))
}

/// List the archive's page entries (per [`COMIC_PAGE_EXTENSIONS`]) in
/// natural-sort reading order. The list's length is the page count; the
/// first entry is the cover page.
pub fn list_pages(path: &Path) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)?;
    let archive = zip::ZipArchive::new(file)?;
    Ok(page_names(&archive))
}

/// Best-effort CBZ page count for `book_id`: the length of the archive's
/// page list. `None` when the book has no CBZ file, the archive is
/// unreadable, or the blocking read fails — detail reads must not fail
/// because one file is broken, so failures log and read as "no count".
/// Shared by the REST detail handler and the web RPC so the two payloads
/// can't disagree.
pub async fn page_count_for_book(pool: &sqlx::SqlitePool, book_id: i64) -> Option<i64> {
    let path = match crate::books::book_file_path(pool, book_id, "CBZ").await {
        Ok(Some(p)) => p,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(book_id, error = %e, "comic page count: file path lookup failed");
            return None;
        }
    };
    match tokio::task::spawn_blocking(move || list_pages(&path)).await {
        Ok(Ok(pages)) => Some(pages.len() as i64),
        Ok(Err(e)) => {
            tracing::warn!(book_id, error = %e, "comic page count: archive unreadable");
            None
        }
        Err(e) => {
            tracing::warn!(book_id, error = %e, "comic page count: task join failed");
            None
        }
    }
}

/// Hard cap on a single decompressed page. A crafted archive can declare a
/// false entry size or lean on deflate's ~1000:1 worst-case ratio to make a
/// small `.cbz` decompress to gigabytes (a "zip bomb") — this bounds memory
/// regardless of what the entry claims. 64 MiB comfortably covers any real
/// scanned page while staying far below an OOM. Same shape as
/// `epub_rewrite::archive::MAX_ENTRY_BYTES`.
#[cfg(not(test))]
const MAX_PAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Test builds use a much smaller cap so the zip-bomb regression test isn't
/// compressing 64 MiB on every run — the bounded-read behavior under test
/// doesn't depend on the cap's exact value.
#[cfg(test)]
const MAX_PAGE_BYTES: u64 = 64 * 1024;

/// Read page `index` (0-based, in [`list_pages`] order) out of the archive:
/// its mime type and decompressed bytes, capped at [`MAX_PAGE_BYTES`].
/// `Ok(None)` when the index is out of range; `Err` when the archive or
/// entry is unreadable or the page exceeds the cap. One entry is
/// decompressed per call — the archive is never extracted whole.
pub fn read_page(path: &Path, index: usize) -> anyhow::Result<Option<(&'static str, Vec<u8>)>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let pages = page_names(&archive);
    let Some(name) = pages.get(index) else {
        return Ok(None);
    };
    let entry = archive.by_name(name)?;
    // Sized by the actual read, not the header's declared size — an
    // attacker-controlled length field must not drive the allocation. One
    // byte past the cap so a page landing exactly on the boundary reads
    // clean while anything larger is distinguishable.
    let mut bytes = Vec::new();
    let read = entry.take(MAX_PAGE_BYTES + 1).read_to_end(&mut bytes)?;
    if read as u64 > MAX_PAGE_BYTES {
        anyhow::bail!("page {name} exceeds {MAX_PAGE_BYTES} byte cap");
    }
    Ok(Some((mime_for_page(name), bytes)))
}

/// Extract the [`IndexedBook`] for a single CBZ file. Mirrors the EPUB
/// parser's failure contract: a malformed archive (not a zip, zero image
/// pages) is logged and surfaced as a per-book error row — never a panic
/// or an aborted scan.
pub fn extract_comic(path: &Path, filename: String, opts: &ScanOptions) -> IndexedBook {
    match read_comic(path) {
        Ok(comic) => indexed_book_from(comic, path, filename, opts),
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "skipping malformed CBZ");
            IndexedBook {
                metadata: EbookMetadata {
                    filename,
                    error: Some(format!("could not open cbz: {e}")),
                    ..Default::default()
                },
                ..Default::default()
            }
        }
    }
}

/// Everything a single pass over the archive yields: the first page's
/// bytes + mime (the cover candidate) and the parsed `ComicInfo.xml`
/// fields when the entry exists.
struct ParsedComic {
    first_page: Option<(String, Vec<u8>)>,
    info: ComicInfo,
}

/// Open the archive once and pull the cover bytes and `ComicInfo.xml`
/// from the same handle. Zero image pages is an error — an archive with
/// nothing to show should surface as a broken row, not an empty book.
fn read_comic(path: &Path) -> anyhow::Result<ParsedComic> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let pages = page_names(&archive);
    if pages.is_empty() {
        anyhow::bail!("no image pages in archive");
    }
    let first_page = read_entry(&mut archive, &pages[0])
        .map(|bytes| (mime_for_page(&pages[0]).to_string(), bytes));
    let info = comic_info_entry(&archive)
        .and_then(|name| read_entry(&mut archive, &name))
        .map(|bytes| parse_comic_info(&bytes))
        .unwrap_or_default();
    Ok(ParsedComic { first_page, info })
}

/// Project a [`ParsedComic`] into the writer's [`IndexedBook`] shape.
/// `ComicInfo.xml` fields win; a missing title falls back to the filename
/// stem, same as the audiobook parser.
fn indexed_book_from(
    comic: ParsedComic,
    path: &Path,
    filename: String,
    opts: &ScanOptions,
) -> IndexedBook {
    let title = comic.info.title.unwrap_or_else(|| filename_stem(&filename));
    let creators = comic
        .info
        .writer
        .as_deref()
        .map(contributors_from_writer)
        .unwrap_or_default();
    let cover = resolve_cover_with(path, opts, || comic.first_page);
    let accent = cover
        .as_ref()
        .and_then(|(_mime, bytes)| extract_accent(bytes));
    IndexedBook {
        metadata: EbookMetadata {
            filename,
            title: Some(title),
            creators,
            series: comic.info.series,
            series_index: comic.info.number,
            accent,
            ..Default::default()
        },
        cover,
        // Stat values get overwritten by `parse_ebook_targets` before the
        // writer sees this struct — same pattern as the EPUB parser.
        mtime_epoch: 0,
        size_bytes: 0,
        // No text to walk — word count is an EPUB-only estimate.
        word_count: None,
    }
}

/// The archive's page entries in natural-sort order. Directory entries and
/// non-image files (including `ComicInfo.xml`) are excluded.
fn page_names<R: Read + std::io::Seek>(archive: &zip::ZipArchive<R>) -> Vec<String> {
    let mut pages: Vec<String> = archive
        .file_names()
        .filter(|name| !name.ends_with('/') && !is_hidden_name(name) && is_page_name(name))
        .map(str::to_string)
        .collect();
    pages.sort_by(|a, b| natural_cmp(a, b));
    pages
}

/// `true` when any path segment starts with `.` — macOS AppleDouble forks
/// (`__MACOSX/._p001.jpg`) and other hidden junk carry image extensions but
/// aren't pages, and their sort keys would land them ahead of the real cover.
fn is_hidden_name(name: &str) -> bool {
    name.split('/').any(|segment| segment.starts_with('.'))
}

fn is_page_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            COMIC_PAGE_EXTENSIONS
                .iter()
                .any(|p| ext.eq_ignore_ascii_case(p))
        })
}

/// Locate the `ComicInfo.xml` entry, matching the basename
/// case-insensitively — real-world archives disagree on both casing and
/// whether the file sits at the root.
fn comic_info_entry<R: Read + std::io::Seek>(archive: &zip::ZipArchive<R>) -> Option<String> {
    archive
        .file_names()
        .find(|name| {
            Path::new(name)
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.eq_ignore_ascii_case("ComicInfo.xml"))
        })
        .map(str::to_string)
}

/// Read one entry's bytes; `None` on any failure so a single corrupt entry
/// degrades to "no cover / no metadata" rather than sinking the book.
fn read_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn mime_for_page(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/jpeg",
    }
}

/// The `ComicInfo.xml` fields the indexer consumes. Everything else in the
/// schema (page dimensions, age rating, …) is ignored.
#[derive(Debug, Default, PartialEq)]
struct ComicInfo {
    title: Option<String>,
    series: Option<String>,
    number: Option<String>,
    writer: Option<String>,
}

/// Best-effort `ComicInfo.xml` parse. Malformed XML yields whatever fields
/// were read before the error — metadata here is advisory, and the
/// filename fallback covers the rest.
fn parse_comic_info(xml: &[u8]) -> ComicInfo {
    use quick_xml::events::Event;

    let mut reader = quick_xml::reader::Reader::from_reader(xml);
    let mut info = ComicInfo::default();
    let mut current: Option<Vec<u8>> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => current = Some(e.name().as_ref().to_vec()),
            Ok(Event::End(_)) => current = None,
            Ok(Event::Text(t)) => {
                if let (Some(tag), Ok(text)) = (current.as_deref(), t.decode()) {
                    let value = text.trim();
                    if !value.is_empty() {
                        let slot = match tag {
                            b"Title" => Some(&mut info.title),
                            b"Series" => Some(&mut info.series),
                            b"Number" => Some(&mut info.number),
                            b"Writer" => Some(&mut info.writer),
                            _ => None,
                        };
                        if let Some(slot) = slot {
                            *slot = Some(value.to_string());
                        }
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
        buf.clear();
    }
    info
}

/// Split the ComicInfo `Writer` field (comma-separated per the spec) into
/// one [`Contributor`] per name.
fn contributors_from_writer(writer: &str) -> Vec<Contributor> {
    writer
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Contributor {
            name: name.to_string(),
            role: None,
            file_as: None,
            id: None,
        })
        .collect()
}

fn filename_stem(filename: &str) -> String {
    Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| filename.to_string())
}

/// Natural-sort comparison for page order: digit runs compare numerically
/// ("p2" < "p10"), everything else compares case-insensitively. Numeric
/// ties fall back to run length so "007" and "7" order deterministically.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ac = a.chars().peekable();
    let mut bc = b.chars().peekable();
    loop {
        match (ac.peek().copied(), bc.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = take_digits(&mut ac);
                let db = take_digits(&mut bc);
                // Compare stripped of leading zeros: more digits wins, then
                // lexical (same length ⇒ digit-wise is numeric ordering).
                let sa = da.trim_start_matches('0');
                let sb = db.trim_start_matches('0');
                let ord = sa
                    .len()
                    .cmp(&sb.len())
                    .then_with(|| sa.cmp(sb))
                    .then_with(|| da.len().cmp(&db.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ac.next();
                bc.next();
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    while let Some(c) = chars.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        out.push(c);
        chars.next();
    }
    out
}

#[cfg(test)]
mod tests;
