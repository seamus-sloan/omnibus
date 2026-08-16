//! EPUB structure extraction for cross-format position mapping: per-spine
//! visible-text stats plus the table of contents (EPUB3 nav.xhtml first,
//! NCX fallback), resolved to spine indices. Runs inside the
//! `BackfillEpubStructure` worker task; persisted by `crate::epub_structure`.

use std::io::{Read, Seek};

use anyhow::Context;
use epub::doc::EpubDoc;
use quick_xml::events::{BytesStart, Event};

#[cfg(test)]
mod tests;

/// One spine item's visible-text measurement, in spine order.
#[derive(Debug, Clone, PartialEq)]
pub struct SpineStat {
    pub spine_index: i64,
    /// OPF-root-relative href, `/`-normalized.
    pub href: String,
    pub visible_chars: i64,
}

/// One table-of-contents entry resolved against the spine.
#[derive(Debug, Clone, PartialEq)]
pub struct TocChapter {
    pub ordinal: i64,
    pub title: String,
    /// The href the TOC pointed at (OPF-root-relative, fragment kept).
    pub href: String,
    pub spine_index: i64,
    /// Cumulative visible chars before the chapter's spine item — the
    /// chapter start as a whole-book text offset, at spine granularity.
    pub start_chars: i64,
}

/// Everything the extraction learns about one EPUB file.
#[derive(Debug, Clone, PartialEq)]
pub struct EpubStructure {
    pub spine: Vec<SpineStat>,
    pub chapters: Vec<TocChapter>,
}

/// Extract spine stats + TOC from an open EPUB. `None` only when the spine
/// is empty or no spine document could be read at all. A book with no
/// usable TOC still returns stats with empty `chapters` — that distinction
/// lets the backfill treat "extracted, honestly TOC-less" as done rather
/// than retrying forever, and keeps fabricated chapters out of the tables.
pub fn extract_structure<R: Read + Seek>(doc: &mut EpubDoc<R>) -> Option<EpubStructure> {
    let idrefs: Vec<String> = doc.spine.iter().map(|s| s.idref.clone()).collect();
    if idrefs.is_empty() {
        return None;
    }
    let mut spine = Vec::new();
    let mut loaded_any = false;
    for (i, idref) in idrefs.iter().enumerate() {
        let href = resource_href(doc, idref).unwrap_or_default();
        // One unreadable document degrades to zero visible text — the same
        // treatment `book_percent` applies — but if nothing loaded there
        // are no stats worth storing.
        let chars = doc
            .get_resource(idref)
            .and_then(|(bytes, _)| crate::kobo_position::visible_chars(&bytes).ok());
        loaded_any |= chars.is_some();
        spine.push(SpineStat {
            spine_index: i as i64,
            href,
            visible_chars: chars.unwrap_or(0) as i64,
        });
    }
    if !loaded_any {
        return None;
    }

    // Nav first: the `epub` crate's `doc.toc` is NCX-only, so an EPUB3
    // shipping only nav.xhtml has an empty `doc.toc` — nav is the primary
    // source and NCX the fallback, not the other way round.
    let raw = nav_toc(doc)
        .filter(|entries| !entries.is_empty())
        .or_else(|| ncx_toc(doc))
        .unwrap_or_default();
    let chapters = resolve_chapters(&spine, raw);
    Some(EpubStructure { spine, chapters })
}

/// Map raw `(title, href)` TOC entries onto the measured spine. Entries
/// that resolve to no spine item are dropped rather than guessed at.
fn resolve_chapters(spine: &[SpineStat], raw: Vec<(String, String)>) -> Vec<TocChapter> {
    let mut cumulative = Vec::with_capacity(spine.len());
    let mut acc: i64 = 0;
    for s in spine {
        cumulative.push(acc);
        acc += s.visible_chars;
    }
    let mut chapters = Vec::new();
    for (title, href) in raw {
        let path = href.split('#').next().unwrap_or("");
        let Some(idx) = spine_index_for(spine, path) else {
            continue;
        };
        chapters.push(TocChapter {
            ordinal: chapters.len() as i64,
            title,
            href,
            spine_index: idx as i64,
            start_chars: cumulative[idx],
        });
    }
    chapters
}

/// Exact OPF-root-relative match first, then a suffix match aligned on a
/// `/` boundary (never mid-segment, so `chapter11.xhtml` can't satisfy
/// `1.xhtml`).
fn spine_index_for(spine: &[SpineStat], path: &str) -> Option<usize> {
    if path.is_empty() {
        return None;
    }
    if let Some(i) = spine.iter().position(|s| s.href == path) {
        return Some(i);
    }
    spine
        .iter()
        .position(|s| segment_suffix_match(&s.href, path))
}

fn segment_suffix_match(a: &str, b: &str) -> bool {
    a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// OPF-root-relative `/`-normalized href of a manifest resource.
fn resource_href<R: Read + Seek>(doc: &EpubDoc<R>, id: &str) -> Option<String> {
    let item = doc.resources.get(id)?;
    let root = doc.root_base.to_string_lossy().replace('\\', "/");
    let mut p = item.path.to_string_lossy().replace('\\', "/");
    if !root.is_empty() {
        let prefix = if root.ends_with('/') {
            root
        } else {
            format!("{root}/")
        };
        if let Some(stripped) = p.strip_prefix(&prefix) {
            p = stripped.to_string();
        }
    }
    Some(p)
}

/// EPUB3 nav TOC: find the manifest resource carrying the `nav` property
/// and parse its `<nav epub:type="toc">` anchors in document order.
fn nav_toc<R: Read + Seek>(doc: &mut EpubDoc<R>) -> Option<Vec<(String, String)>> {
    let nav_id = doc
        .resources
        .iter()
        .find(|(_, item)| {
            item.properties
                .as_deref()
                .is_some_and(|p| p.split_whitespace().any(|t| t == "nav"))
        })
        .map(|(id, _)| id.clone())?;
    let nav_href = resource_href(doc, &nav_id).unwrap_or_default();
    let nav_dir = nav_href
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default();
    let (bytes, _) = doc.get_resource(&nav_id)?;
    match parse_nav(&bytes, &nav_dir) {
        Ok(entries) => Some(entries),
        Err(e) => {
            tracing::warn!(error = %e, "nav.xhtml parse failed; falling back to NCX");
            None
        }
    }
}

/// Pull `(title, href)` pairs out of `<nav epub:type="toc">` in document
/// order; hrefs come back OPF-root-relative (joined against the nav
/// document's own directory), fragments preserved. `Err` only on
/// unparseable XML.
fn parse_nav(xhtml: &[u8], nav_dir: &str) -> anyhow::Result<Vec<(String, String)>> {
    let mut reader = quick_xml::Reader::from_reader(xhtml);
    let mut buf = Vec::new();
    let mut entries = Vec::new();
    // 0 = not inside the toc nav; >0 = depth of nested <nav> inside it.
    let mut in_toc_nav = false;
    let mut nested_navs = 0usize;
    let mut current: Option<(String, String)> = None;
    loop {
        match reader.read_event_into(&mut buf).context("parse nav")? {
            Event::Start(ref e) => {
                let local = e.local_name();
                if local.as_ref().eq_ignore_ascii_case(b"nav") {
                    if in_toc_nav {
                        nested_navs += 1;
                    } else if nav_is_toc(e) {
                        in_toc_nav = true;
                    }
                } else if in_toc_nav && local.as_ref().eq_ignore_ascii_case(b"a") {
                    let href = e
                        .try_get_attribute("href")
                        .ok()
                        .flatten()
                        .and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .ok()
                                .map(|v| v.into_owned())
                        })
                        .unwrap_or_default();
                    current = Some((join_href(nav_dir, &href), String::new()));
                }
            }
            Event::Text(ref t) => {
                if let Some((_, title)) = current.as_mut() {
                    title.push_str(&t.decode().context("decode nav text")?);
                }
            }
            Event::GeneralRef(ref r) => {
                if let Some((_, title)) = current.as_mut() {
                    if let Some(c) = r.resolve_char_ref().ok().flatten().or(match r.as_ref() {
                        b"amp" => Some('&'),
                        b"lt" => Some('<'),
                        b"gt" => Some('>'),
                        b"apos" => Some('\''),
                        b"quot" => Some('"'),
                        b"nbsp" => Some('\u{a0}'),
                        _ => None,
                    }) {
                        title.push(c);
                    }
                }
            }
            Event::End(ref e) => {
                let local = e.local_name();
                if local.as_ref().eq_ignore_ascii_case(b"a") {
                    if let Some((href, title)) = current.take() {
                        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !href.is_empty() {
                            entries.push((title, href));
                        }
                    }
                } else if local.as_ref().eq_ignore_ascii_case(b"nav") && in_toc_nav {
                    if nested_navs > 0 {
                        nested_navs -= 1;
                    } else {
                        break;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(entries)
}

/// Whether a `<nav>` start tag carries `epub:type` (any-namespace `type`)
/// with the `toc` token.
fn nav_is_toc(e: &BytesStart) -> bool {
    e.attributes().flatten().any(|a| {
        let key = a.key.as_ref();
        let local = key.rsplit(|b| *b == b':').next().unwrap_or(key);
        local.eq_ignore_ascii_case(b"type")
            && a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .is_ok_and(|v| v.split_whitespace().any(|t| t.eq_ignore_ascii_case("toc")))
    })
}

/// NCX fallback: the `epub` crate has already parsed `toc.ncx` into
/// `doc.toc` (it never reads nav.xhtml, which is why nav goes first).
/// Flattened depth-first in document order; `content` paths arrive
/// root_base-joined, so strip that prefix the way hrefs are normalized.
fn ncx_toc<R: Read + Seek>(doc: &EpubDoc<R>) -> Option<Vec<(String, String)>> {
    if doc.toc.is_empty() {
        return None;
    }
    let root = doc.root_base.to_string_lossy().replace('\\', "/");
    let prefix = if root.is_empty() {
        String::new()
    } else if root.ends_with('/') {
        root
    } else {
        format!("{root}/")
    };
    let mut out = Vec::new();
    flatten_navpoints(&doc.toc, &prefix, &mut out);
    Some(out)
}

fn flatten_navpoints(
    points: &[epub::doc::NavPoint],
    root_prefix: &str,
    out: &mut Vec<(String, String)>,
) {
    for p in points {
        let mut path = p.content.to_string_lossy().replace('\\', "/");
        if !root_prefix.is_empty() {
            if let Some(stripped) = path.strip_prefix(root_prefix) {
                path = stripped.to_string();
            }
        }
        out.push((p.label.trim().to_string(), path));
        flatten_navpoints(&p.children, root_prefix, out);
    }
}

/// Join a nav-document-relative href onto the nav document's directory,
/// resolving `.`/`..` segments and preserving any fragment.
fn join_href(dir: &str, href: &str) -> String {
    let (path, frag) = match href.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (href, None),
    };
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut joined = parts.join("/");
    if let Some(f) = frag {
        joined.push('#');
        joined.push_str(f);
    }
    joined
}
