//! In-place OPF `<metadata>` rewrite (F5.8 #1372). Copies the source OPF
//! through byte-for-byte via quick-xml events, dropping the descriptive
//! `<dc:*>` elements (and the two `calibre:series` metas) and re-emitting them
//! from the book's effective metadata. Everything else — the package
//! `unique-identifier`, `<dc:identifier>`s, the `<meta name="cover">` pointer,
//! refinements, and the whole manifest/spine — is preserved untouched, so the
//! EPUB's identity and structure survive.

use anyhow::Context;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use omnibus_shared::EbookMetadata;

use crate::opf_export::xml_escape;

/// Descriptive `<dc:*>` local names we own: their originals are dropped and a
/// fresh copy is emitted from the effective metadata. Everything not listed
/// (`identifier`, `rights`, `source`, `type`, …) is preserved verbatim.
///
/// `contributor` is dropped alongside `creator` because the effective
/// `creators` vec already folds contributors in (mirroring `render_opf`), so
/// re-emitting them as `<dc:creator>` would otherwise duplicate them.
const MANAGED_DC: &[&[u8]] = &[
    b"title",
    b"creator",
    b"contributor",
    b"description",
    b"publisher",
    b"date",
    b"language",
    b"subject",
];

/// Rewrite `opf` so its `<metadata>` reflects `book`'s effective values.
///
/// Returns the transformed OPF bytes. The manifest, spine, guide, and all
/// preserved metadata elements are copied through unchanged. Fails only on
/// malformed XML the source reader rejects.
pub(super) fn transform_opf(opf: &[u8], book: &EbookMetadata) -> anyhow::Result<Vec<u8>> {
    let mut reader = Reader::from_reader(opf);
    let mut writer = Writer::new(Vec::with_capacity(opf.len() + 256));

    let mut depth: i32 = 0;
    let mut metadata_depth: Option<i32> = None;
    // When `Some(d)`, we're inside a managed element's subtree opened at depth
    // `d`; swallow every event until its matching end at `d`.
    let mut skip_depth: Option<i32> = None;
    let mut buf = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .with_context(|| format!("parse OPF at byte {}", reader.buffer_position()))?;
        match event {
            Event::Eof => break,

            Event::Start(e) => {
                depth += 1;
                if skip_depth.is_some() {
                    // Inside a dropped subtree — swallow.
                } else if metadata_depth.is_none() && is_metadata(e.name().as_ref()) {
                    metadata_depth = Some(depth);
                    writer.write_event(Event::Start(e.borrow()))?;
                } else if metadata_depth == Some(depth - 1) && is_managed(&e) {
                    skip_depth = Some(depth);
                } else {
                    writer.write_event(Event::Start(e.borrow()))?;
                }
            }

            Event::Empty(e) => {
                // Self-closing element — no depth change. Managed empties (e.g.
                // `<meta name="calibre:series" .../>`, `<dc:date/>`) are dropped;
                // everything else (notably `<meta name="cover" .../>`) is kept.
                let managed = metadata_depth == Some(depth) && is_managed(&e);
                if skip_depth.is_none() && !managed {
                    writer.write_event(Event::Empty(e.borrow()))?;
                }
            }

            Event::End(e) => {
                if let Some(d) = skip_depth {
                    if depth == d {
                        skip_depth = None;
                    }
                    depth -= 1;
                    continue;
                }
                if metadata_depth == Some(depth) {
                    // Closing `<metadata>` — inject the regenerated children
                    // just before the end tag, then fall through to write it.
                    writer
                        .get_mut()
                        .extend_from_slice(render_managed(book).as_bytes());
                    metadata_depth = None;
                }
                writer.write_event(Event::End(e.borrow()))?;
                depth -= 1;
            }

            other => {
                if skip_depth.is_none() {
                    writer.write_event(other.borrow())?;
                }
            }
        }
        buf.clear();
    }

    Ok(writer.into_inner())
}

/// True for the OPF `<metadata>` container by local name (prefix-agnostic:
/// `<metadata>` or `<opf:metadata>`).
fn is_metadata(qname: &[u8]) -> bool {
    local_name(qname) == b"metadata"
}

/// Whether an element is one we regenerate: a managed `<dc:*>` element, or a
/// `<meta name="calibre:series"|"calibre:series_index">` (both the paired and
/// self-closing spellings reach here).
fn is_managed(e: &quick_xml::events::BytesStart) -> bool {
    let name = e.name();
    let local = local_name(name.as_ref());
    if local == b"meta" {
        let meta = meta_name(e);
        return matches!(
            meta.as_deref(),
            Some(b"calibre:series") | Some(b"calibre:series_index")
        );
    }
    MANAGED_DC.contains(&local)
}

/// The `name` attribute of a `<meta>` element, if present.
fn meta_name(e: &quick_xml::events::BytesStart) -> Option<Vec<u8>> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == b"name")
        .map(|a| a.value.into_owned())
}

/// Strip an XML prefix (`dc:title` → `title`).
fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

/// Render the managed `<metadata>` children from effective values, using the
/// conventional `dc:`/`opf:` prefixes (declared by every OPF we rewrite).
/// Indented four spaces to sit inside `<metadata>`.
fn render_managed(book: &EbookMetadata) -> String {
    let mut out = String::new();

    // dc:title is mandatory in OPF 2.0 — fall back to the filename like the UI.
    let title = book
        .title
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&book.filename);
    out.push_str(&format!("    <dc:title>{}</dc:title>\n", xml_escape(title)));

    for creator in &book.creators {
        let mut attrs = String::new();
        if let Some(role) = creator.role.as_deref().filter(|s| !s.is_empty()) {
            attrs.push_str(&format!(" opf:role=\"{}\"", xml_escape(role)));
        }
        if let Some(file_as) = creator.file_as.as_deref().filter(|s| !s.is_empty()) {
            attrs.push_str(&format!(" opf:file-as=\"{}\"", xml_escape(file_as)));
        }
        out.push_str(&format!(
            "    <dc:creator{}>{}</dc:creator>\n",
            attrs,
            xml_escape(&creator.name)
        ));
    }

    push_dc(&mut out, "description", book.description.as_deref());
    push_dc(&mut out, "publisher", book.publisher.as_deref());
    push_dc(&mut out, "date", book.published.as_deref());
    push_dc(&mut out, "language", book.language.as_deref());

    for subject in &book.subjects {
        out.push_str(&format!(
            "    <dc:subject>{}</dc:subject>\n",
            xml_escape(subject)
        ));
    }

    if let Some(series) = book.series.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "    <meta name=\"calibre:series\" content=\"{}\"/>\n",
            xml_escape(series)
        ));
    }
    if let Some(index) = book.series_index.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "    <meta name=\"calibre:series_index\" content=\"{}\"/>\n",
            xml_escape(index)
        ));
    }

    out
}

/// Emit `<dc:{tag}>{value}</dc:{tag}>` when `value` is present and non-empty.
fn push_dc(out: &mut String, tag: &str, value: Option<&str>) {
    if let Some(v) = value.filter(|s| !s.is_empty()) {
        out.push_str(&format!("    <dc:{tag}>{}</dc:{tag}>\n", xml_escape(v)));
    }
}
