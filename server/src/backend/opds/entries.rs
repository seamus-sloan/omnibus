//! Shared acquisition-entry builder: turn one book's `EbookMetadata` row
//! into an OPDS `<entry>`. Wires up download / cover / thumbnail links that
//! point at this router's Basic-auth'd [`super::delegate`] routes (which
//! call the `/api/ebooks/*` and `/api/covers|thumbs/*` handler bodies)
//! rather than duplicating their resolution logic.

use omnibus_shared::EbookMetadata;

use super::atom::{Entry, Link};
use super::entry_updated;

/// Wire mime for a served CBZ archive — mirrors `ebooks::CBZ_MIME`, kept as
/// its own constant since that one is `pub(super)` to the `backend` module,
/// not reachable from here. `pub(super)` so `opds::json_entries` can build
/// the same acquisition link without duplicating the mime string.
pub(super) const CBZ_MIME: &str = "application/vnd.comicbook+zip";

/// Whether a `book_files` format string is one this catalog offers as a
/// download — EPUB/CBZ, what the `/opds/ebooks/{uuid}/{file,download}`
/// delegates serve. The **single** predicate for both catalogs; the author
/// and series feeds used to carry local copies of it.
pub(super) fn is_ereader_format(format: &str) -> bool {
    format.eq_ignore_ascii_case("epub") || format.eq_ignore_ascii_case("cbz")
}

/// Drop every book without an e-reader-servable file. The shared list
/// queries surface physical-only books on purpose for the web UI (#1181),
/// but in a catalog for e-readers a row with no usable acquisition link is
/// dead weight (#1811). Strictly EPUB/CBZ: audiobook-only books are
/// excluded too, so [`download_link`]'s audio fallback arms never fire
/// from a feed — they remain only for defense on unfiltered callers.
/// Every feed builder calls this right after its fetch, so both catalogs
/// filter identically.
pub(super) fn retain_ereader_books(books: &mut Vec<EbookMetadata>) {
    books.retain(|b| b.formats.iter().any(|f| is_ereader_format(f)));
}

/// Build the acquisition `<entry>` for one book: title, id, authors,
/// summary, and — when the book has a format this catalog knows how to
/// link to — download, cover, and thumbnail links.
pub(super) fn book_entry(book: &EbookMetadata) -> Entry {
    let uuid = book.unique_identifier.as_deref().filter(|u| !u.is_empty());
    let mut links = Vec::new();
    if let Some(uuid) = uuid {
        if let Some((href, type_)) = download_link(uuid, book) {
            links.push(Link::new("http://opds-spec.org/acquisition", href, type_));
        }
        links.push(Link::new(
            "http://opds-spec.org/image",
            format!("/opds/covers/{uuid}"),
            "image/jpeg",
        ));
        links.push(Link::new(
            "http://opds-spec.org/image/thumbnail",
            format!("/opds/thumbs/{uuid}/sm"),
            "image/webp",
        ));
    }
    let id = uuid.map_or_else(
        || format!("urn:omnibus:opds:book:{}", book.id),
        |uuid| format!("urn:uuid:{uuid}"),
    );
    Entry {
        id,
        title: book.display_title(),
        updated: entry_updated(book.added_at.as_deref()),
        summary: book.description.clone(),
        authors: book.creators.iter().map(|c| c.name.clone()).collect(),
        links,
    }
}

/// The acquisition download link `(href, mime)` for `book`, chosen by its
/// first matching known format. Mirrors the fallback order
/// `/api/ebooks/{uuid}/file` already uses (EPUB, else CBZ — see that
/// handler's doc comment), extended with the audiobook formats so a
/// dual-format or audiobook-only entry still links somewhere. `None` when
/// the book carries no format this catalog can link to (e.g. a
/// physical-only entry with no files). Returns a bare tuple rather than an
/// [`atom::Link`](super::atom::Link) so `opds::json_entries` can build the
/// same link as its own JSON `Link` type — the whole point of factoring
/// this out is one format-selection decision feeding both catalogs.
pub(super) fn download_link(uuid: &str, book: &EbookMetadata) -> Option<(String, &'static str)> {
    let has = |fmt: &str| book.formats.iter().any(|f| f.eq_ignore_ascii_case(fmt));
    if has("epub") {
        return Some((
            format!("/opds/ebooks/{uuid}/download"),
            "application/epub+zip",
        ));
    }
    if has("cbz") {
        // `/download` is EPUB-only (see `ebooks::get_ebook_download`); a
        // comic-only book's whole-file read lives at `/file` instead.
        return Some((format!("/opds/ebooks/{uuid}/file"), CBZ_MIME));
    }
    if has("m4b") || has("m4a") {
        return Some((format!("/opds/audiobooks/{uuid}/download"), "audio/mp4"));
    }
    if has("mp3") {
        return Some((format!("/opds/audiobooks/{uuid}/download"), "audio/mpeg"));
    }
    None
}
