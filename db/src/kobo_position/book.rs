//! EPUB container glue for the span↔CFI translation: open a book, map a
//! `Location.Source` href to a spine index, and read spine documents. Both
//! the source EPUB and the cached KEPUB are ordinary EPUB containers, so
//! one set of helpers serves both sides.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::Context;
use epub::doc::EpubDoc;

pub(super) type Doc = EpubDoc<BufReader<File>>;

/// Open an EPUB (or KEPUB) container from disk.
pub(super) fn open_doc(path: &Path) -> anyhow::Result<Doc> {
    EpubDoc::new(path).with_context(|| format!("open epub {}", path.display()))
}

/// The OPF-relative href of spine item `i` — the string a KoboSpan
/// `Location.Source` carries.
pub(super) fn spine_href(doc: &Doc, i: usize) -> Option<String> {
    let idref = &doc.spine.get(i)?.idref;
    let res = doc.resources.get(idref)?;
    let rel = res.path.strip_prefix(&doc.root_base).unwrap_or(&res.path);
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Resolve a `Location.Source` href to its spine index. Exact
/// OPF-relative match first, then a suffix match for containers whose
/// Source carries extra leading path segments.
pub(super) fn spine_index_for_source(doc: &Doc, source: &str) -> Option<usize> {
    let normalized = source.trim_start_matches("./");
    let exact = (0..doc.spine.len()).find(|&i| spine_href(doc, i).as_deref() == Some(normalized));
    exact.or_else(|| {
        (0..doc.spine.len()).find(|&i| {
            spine_href(doc, i)
                .is_some_and(|h| h.ends_with(normalized) || normalized.ends_with(h.as_str()))
        })
    })
}

/// Read spine item `i`'s document bytes.
pub(super) fn read_spine_entry(doc: &mut Doc, i: usize) -> anyhow::Result<Vec<u8>> {
    let idref = doc
        .spine
        .get(i)
        .map(|s| s.idref.clone())
        .with_context(|| format!("spine index {i} out of range"))?;
    doc.get_resource(&idref)
        .map(|(bytes, _mime)| bytes)
        .with_context(|| format!("read spine entry {idref}"))
}
