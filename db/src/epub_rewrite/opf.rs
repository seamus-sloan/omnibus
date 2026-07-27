//! In-place OPF `<metadata>` rewrite (F5.8 #1372). Copies the source OPF
//! through byte-for-byte via quick-xml events, dropping the descriptive
//! `<dc:*>` elements (and the two `calibre:series` metas) and re-emitting them
//! from the book's effective metadata. Everything else — the package
//! `unique-identifier`, `<dc:identifier>`s, the `<meta name="cover">` pointer,
//! refinements, and the whole manifest/spine — is preserved untouched, so the
//! EPUB's identity and structure survive.

use anyhow::Context;
use omnibus_shared::EbookMetadata;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::opf_export::render_effective_metadata;

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

/// Nesting state threaded through `transform_opf`'s per-event loop: current
/// depth, which depth (if any) the source `<metadata>` element opened at,
/// which depth (if any) a dropped managed subtree opened at, and whether the
/// `dc:`/`opf:` prefixes we're about to inject are bound by the source.
struct RewriteState {
    depth: i32,
    metadata_depth: Option<i32>,
    // When `Some(d)`, we're inside a managed element's subtree opened at
    // depth `d`; swallow every event until its matching end at `d`.
    skip_depth: Option<i32>,
    dc_bound: bool,
    opf_bound: bool,
}

impl RewriteState {
    fn new() -> Self {
        Self {
            depth: 0,
            metadata_depth: None,
            skip_depth: None,
            dc_bound: false,
            opf_bound: false,
        }
    }
}

/// Rewrite `opf` so its `<metadata>` reflects `book`'s effective values.
///
/// Returns the transformed OPF bytes. The manifest, spine, guide, and all
/// preserved metadata elements are copied through unchanged. Fails only on
/// malformed XML the source reader rejects.
pub(super) fn transform_opf(opf: &[u8], book: &EbookMetadata) -> anyhow::Result<Vec<u8>> {
    let mut reader = Reader::from_reader(opf);
    let mut writer = Writer::new(Vec::with_capacity(opf.len() + 256));
    let mut state = RewriteState::new();
    // We inject `dc:`/`opf:`-prefixed markup, so those prefixes must be bound by
    // the source OPF; `needs_opf` gates whether the `opf:` check applies.
    let needs_opf = creator_needs_opf_attrs(book);
    let mut buf = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .with_context(|| format!("parse OPF at byte {}", reader.buffer_position()))?;
        match event {
            Event::Eof => break,
            Event::Start(e) => handle_start(e, &mut writer, &mut state)?,
            Event::Empty(e) => handle_empty(e, &mut writer, &state)?,
            Event::End(e) => {
                if let Some(d) = state.skip_depth {
                    if state.depth == d {
                        state.skip_depth = None;
                    }
                    state.depth -= 1;
                    continue;
                }
                inject_managed_metadata_before_close(&mut writer, &mut state, book, needs_opf)?;
                writer.write_event(Event::End(e.borrow()))?;
                state.depth -= 1;
            }
            other => {
                if state.skip_depth.is_none() {
                    writer.write_event(other.borrow())?;
                }
            }
        }
        buf.clear();
    }

    Ok(writer.into_inner())
}

/// Whether any creator carries a role or file-as that would render as an
/// `opf:role`/`opf:file-as` attribute, requiring the `xmlns:opf` prefix.
fn creator_needs_opf_attrs(book: &EbookMetadata) -> bool {
    book.creators.iter().any(|c| {
        c.role.as_deref().is_some_and(|s| !s.is_empty())
            || c.file_as.as_deref().is_some_and(|s| !s.is_empty())
    })
}

/// `Event::Start` handling: track depth and (while still outside
/// `<metadata>`'s subtree) namespace bindings, then either swallow (inside a
/// dropped subtree), open the tracked `<metadata>`, start dropping a managed
/// element's subtree, or pass the element through.
fn handle_start(
    e: BytesStart<'_>,
    writer: &mut Writer<Vec<u8>>,
    state: &mut RewriteState,
) -> anyhow::Result<()> {
    state.depth += 1;
    // Only elements at or above `<metadata>` are still in scope at the
    // injection point (just before `</metadata>`): once `metadata_depth` is
    // set, we're inside its subtree, where an XML namespace declaration goes
    // out of scope again as soon as the declaring element closes.
    if state.metadata_depth.is_none() {
        track_namespace_bindings(&e, state);
    }

    if state.skip_depth.is_some() {
        // Inside a dropped subtree — swallow.
    } else if state.metadata_depth.is_none() && is_metadata(e.name().as_ref()) {
        state.metadata_depth = Some(state.depth);
        writer.write_event(Event::Start(e.borrow()))?;
    } else if state.metadata_depth == Some(state.depth - 1) && is_managed(&e) {
        state.skip_depth = Some(state.depth);
    } else {
        writer.write_event(Event::Start(e.borrow()))?;
    }
    Ok(())
}

/// `Event::Empty` handling: self-closing elements don't change depth. Managed
/// empties (e.g. `<meta name="calibre:series" .../>`, `<dc:date/>`) are
/// dropped; everything else (notably `<meta name="cover" .../>`) is kept.
fn handle_empty(
    e: BytesStart<'_>,
    writer: &mut Writer<Vec<u8>>,
    state: &RewriteState,
) -> anyhow::Result<()> {
    let managed = state.metadata_depth == Some(state.depth) && is_managed(&e);
    if state.skip_depth.is_none() && !managed {
        writer.write_event(Event::Empty(e.borrow()))?;
    }
    Ok(())
}

/// Record whether `e` declares the `xmlns:dc`/`xmlns:opf` prefixes the
/// rewrite is about to inject. Callers must only invoke this for `<metadata>`
/// itself or one of its ancestors — a declaration on a descendant (e.g. a
/// redundant `xmlns:dc` on a `<dc:title>` child) is out of scope again by the
/// time we inject new markup just before `</metadata>`, so trusting it would
/// let unbound `dc:`/`opf:` elements slip into invalid OPF.
fn track_namespace_bindings(e: &BytesStart<'_>, state: &mut RewriteState) {
    if declares(e, b"xmlns:dc") {
        state.dc_bound = true;
    }
    if declares(e, b"xmlns:opf") {
        state.opf_bound = true;
    }
}

/// When the upcoming `Event::End` closes the tracked `<metadata>` element,
/// inject the regenerated managed children just before it. No-op otherwise.
///
/// Refuses (bails) when a prefix the injected markup would use isn't bound
/// by the source, so the caller falls back to the untouched source EPUB
/// instead of shipping invalid XML.
fn inject_managed_metadata_before_close(
    writer: &mut Writer<Vec<u8>>,
    state: &mut RewriteState,
    book: &EbookMetadata,
    needs_opf: bool,
) -> anyhow::Result<()> {
    if state.metadata_depth != Some(state.depth) {
        return Ok(());
    }
    if !state.dc_bound {
        anyhow::bail!("OPF does not bind xmlns:dc; refusing to inject dc:* metadata");
    }
    if needs_opf && !state.opf_bound {
        anyhow::bail!("OPF does not bind xmlns:opf; refusing to inject opf:* attributes");
    }
    writer
        .get_mut()
        .extend_from_slice(render_effective_metadata(book).as_bytes());
    state.metadata_depth = None;
    Ok(())
}

/// Whether `e` declares the given namespace attribute (e.g. `xmlns:dc`).
fn declares(e: &BytesStart, attr: &[u8]) -> bool {
    e.attributes().flatten().any(|a| a.key.as_ref() == attr)
}

/// True for the OPF `<metadata>` container by local name (prefix-agnostic:
/// `<metadata>` or `<opf:metadata>`).
fn is_metadata(qname: &[u8]) -> bool {
    local_name(qname) == b"metadata"
}

/// Whether an element is one we regenerate: a managed `<dc:*>` element, or a
/// `<meta name="calibre:series"|"calibre:series_index">` (both the paired and
/// self-closing spellings reach here).
fn is_managed(e: &BytesStart) -> bool {
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
fn meta_name(e: &BytesStart) -> Option<Vec<u8>> {
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
