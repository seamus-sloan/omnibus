//! Single-pass XHTML walker backing the span↔CFI translation. One
//! `FileIndex` build per content document answers every query the module
//! needs; the invariants below are the correctness argument for the whole
//! translation, so a change to any of them must hold on *both* the kepub
//! and the source walks or aligned offsets silently diverge.
//!
//! Invariants:
//! - **Normalized text stream.** All character data under `<body>`,
//!   entity-decoded, with every whitespace run collapsed to one space and
//!   leading whitespace dropped. Offsets count Unicode scalars in this
//!   stream. kepubify's injected markup (`koboSpan` wrappers, the
//!   `book-columns` divs) contributes no characters, so the kepub and
//!   source streams are identical whenever the underlying text is.
//! - **CFI step accounting** mirrors epub.js, the only consumer: element
//!   child k (1-based) is even step `2k`; consecutive character-data nodes
//!   coalesce into one text cluster whose odd index is `2t+1` after t
//!   *earlier text clusters in the same parent* — epub.js numbers text
//!   nodes among text siblings only, which diverges from the CFI spec's
//!   interleaved numbering whenever an element precedes the cluster
//!   (comments/PIs neither count nor split a cluster). Terminal offsets
//!   are UTF-16 code units into the cluster's raw (uncollapsed) text.
//! - **Anchors land on visible characters.** A span's position is the first
//!   non-space normalized character at/after its start tag; a normalized
//!   offset falling in collapsed whitespace resolves to the next visible
//!   character. Every anchor carries a snippet of the following normalized
//!   text so the two walks can prove they aligned.

use anyhow::Context;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use super::cfi::CfiTail;
use super::location::{parse_span_id, SpanPoint};

/// Normalized characters of following text captured per anchor; the two
/// sides of a derivation must produce byte-identical snippets or the
/// result is discarded.
pub(super) const SNIPPET_CHARS: usize = 32;

/// One coalesced text cluster in the source DOM: its CFI address plus the
/// raw text and enough state to replay normalization inside it.
struct Cluster {
    element_steps: Vec<u32>,
    text_index: u32,
    /// Normalized-stream offset of the first character this cluster emitted.
    norm_start: u64,
    /// Chars this cluster emitted into the normalized stream.
    norm_len: u64,
    /// Whitespace-collapse state on entry: a pending run was already open.
    entered_in_run: bool,
    /// Raw decoded text (uncollapsed) — the DOM text node content.
    raw: String,
}

/// A `kobo.N.M` span start observed in a kepub document.
struct SpanMark {
    n: u32,
    m: u32,
    /// Normalized offset of the first visible character at/after the span.
    norm_start: u64,
}

/// Everything the four queries need from one content document.
pub(super) struct FileIndex {
    norm: String,
    clusters: Vec<Cluster>,
    spans: Vec<SpanMark>,
}

/// Build the index for one XHTML document. `Err` only on unparseable
/// input — the caller logs and abandons the derivation.
pub(super) fn index_file(xhtml: &[u8]) -> anyhow::Result<FileIndex> {
    let mut reader = Reader::from_reader(xhtml);
    let mut b = Builder::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf).context("parse xhtml")? {
            Event::Start(ref e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let id = e.try_get_attribute("id").ok().flatten().and_then(|a| {
                    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .ok()
                        .map(|v| v.into_owned())
                });
                b.element_start(&local, id.as_deref());
            }
            Event::Empty(ref e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                b.element_start(&local, None);
                b.element_end(&local);
            }
            Event::End(ref e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                b.element_end(&local);
            }
            Event::Text(ref t) => {
                let text = t.decode().context("decode text")?;
                b.chars(&text);
            }
            Event::CData(ref t) => {
                let text = t.decode().context("decode cdata")?;
                b.chars(&text);
            }
            Event::GeneralRef(ref r) => {
                if let Some(c) = resolve_entity(r.as_ref(), r.resolve_char_ref().ok().flatten()) {
                    b.chars(&c.to_string());
                }
            }
            // Comments, PIs, declarations and doctypes neither emit text
            // nor close a cluster (CFI coalescing ignores them).
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) | Event::DocType(_) => {}
            Event::Eof => break,
        }
        buf.clear();
    }
    Ok(b.finish())
}

/// Resolve a general entity reference to its character. Numeric refs come
/// pre-resolved from quick-xml; named refs cover the XML five plus the
/// XHTML names common in ebook prose. Unknown names yield nothing — if the
/// two sides disagree on one, the snippet check catches it.
fn resolve_entity(name: &[u8], char_ref: Option<char>) -> Option<char> {
    if let Some(c) = char_ref {
        return Some(c);
    }
    Some(match name {
        b"amp" => '&',
        b"lt" => '<',
        b"gt" => '>',
        b"apos" => '\'',
        b"quot" => '"',
        b"nbsp" => '\u{a0}',
        b"mdash" => '\u{2014}',
        b"ndash" => '\u{2013}',
        b"hellip" => '\u{2026}',
        b"lsquo" => '\u{2018}',
        b"rsquo" => '\u{2019}',
        b"ldquo" => '\u{201c}',
        b"rdquo" => '\u{201d}',
        b"shy" => '\u{ad}',
        _ => return None,
    })
}

#[derive(Default)]
struct Builder {
    norm: String,
    norm_len: u64,
    /// Depth of `<body>` nesting (0 = outside; text outside body is invisible).
    body_depth: u32,
    /// Per-open-element count of element children seen so far. One frame
    /// per open element, root included.
    child_counts: Vec<u32>,
    /// Per-open-element count of text clusters closed or open so far —
    /// the epub.js text-index counter (text nodes numbered among text
    /// siblings only). Frames mirror `child_counts`.
    text_counts: Vec<u32>,
    /// Element steps from the root element's children down to the open
    /// element — `<body>` itself contributes the leading step (its
    /// position among `<html>`'s children), matching the CFI tail path.
    steps: Vec<u32>,
    /// Whitespace-collapse state: inside a run whose space was emitted.
    in_run: bool,
    /// Nothing visible emitted yet (leading whitespace is dropped).
    emitted_any: bool,
    clusters: Vec<Cluster>,
    /// Open cluster index, if the previous sibling event was character data.
    open_cluster: Option<usize>,
    spans: Vec<SpanMark>,
    /// Span ids seen whose first visible character hasn't arrived yet.
    pending_spans: Vec<(u32, u32)>,
}

impl Builder {
    fn element_start(&mut self, local: &str, id: Option<&str>) {
        self.open_cluster = None;
        // The root element carries no step (the CFI tail starts at its
        // children); every other element is step 2k among its siblings.
        if let Some(count) = self.child_counts.last_mut() {
            *count += 1;
            self.steps.push(*count * 2);
        }
        if self.body_depth > 0 {
            if let Some((n, m)) = id.and_then(parse_span_id) {
                self.pending_spans.push((n, m));
            }
        }
        if local.eq_ignore_ascii_case("body") || self.body_depth > 0 {
            self.body_depth += 1;
        }
        self.child_counts.push(0);
        self.text_counts.push(0);
    }

    fn element_end(&mut self, _local: &str) {
        self.open_cluster = None;
        self.child_counts.pop();
        self.text_counts.pop();
        if !self.child_counts.is_empty() {
            self.steps.pop();
        }
        if self.body_depth > 0 {
            self.body_depth -= 1;
        }
    }

    fn chars(&mut self, text: &str) {
        if self.body_depth == 0 {
            return;
        }
        let cluster = match self.open_cluster {
            Some(i) => i,
            None => {
                let clusters_so_far = self.text_counts.last().copied().unwrap_or(0);
                if let Some(t) = self.text_counts.last_mut() {
                    *t += 1;
                }
                self.clusters.push(Cluster {
                    element_steps: self.steps.clone(),
                    text_index: clusters_so_far * 2 + 1,
                    norm_start: self.norm_len,
                    norm_len: 0,
                    entered_in_run: self.in_run || !self.emitted_any,
                    raw: String::new(),
                });
                self.clusters.len() - 1
            }
        };
        self.open_cluster = Some(cluster);
        self.clusters[cluster].raw.push_str(text);
        for c in text.chars() {
            if c.is_whitespace() {
                if !self.in_run && self.emitted_any {
                    self.norm.push(' ');
                    self.norm_len += 1;
                    self.clusters[cluster].norm_len += 1;
                }
                self.in_run = true;
            } else {
                self.norm.push(c);
                self.norm_len += 1;
                self.clusters[cluster].norm_len += 1;
                self.in_run = false;
                self.emitted_any = true;
                for (n, m) in self.pending_spans.drain(..) {
                    self.spans.push(SpanMark {
                        n,
                        m,
                        norm_start: self.norm_len - 1,
                    });
                }
            }
        }
    }

    fn finish(mut self) -> FileIndex {
        // Trailing whitespace run: the emitted space is not a landable
        // anchor; leave it (offsets landing there resolve to EOF).
        let end = self.norm_len;
        for (n, m) in self.pending_spans.drain(..) {
            self.spans.push(SpanMark {
                n,
                m,
                norm_start: end,
            });
        }
        FileIndex {
            norm: self.norm,
            clusters: self.clusters,
            spans: self.spans,
        }
    }
}

/// An anchored position: normalized offset plus the proving snippet.
pub(super) struct Anchor {
    pub norm_offset: u64,
    pub snippet: String,
}

impl FileIndex {
    /// Total visible characters (normalized stream length).
    pub(super) fn visible_char_count(&self) -> u64 {
        self.norm.chars().count() as u64
    }

    /// Snippet of up to [`SNIPPET_CHARS`] normalized chars from `offset`.
    pub(super) fn snippet_at(&self, offset: u64) -> String {
        self.norm
            .chars()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(SNIPPET_CHARS)
            .collect()
    }

    /// Kepub walk: where does span `kobo.N.M` start?
    pub(super) fn span_start(&self, n: u32, m: u32) -> Option<Anchor> {
        let mark = self.spans.iter().find(|s| s.n == n && s.m == m)?;
        Some(Anchor {
            norm_offset: mark.norm_start,
            snippet: self.snippet_at(mark.norm_start),
        })
    }

    /// Kepub walk: the normalized boundary offset for a DOM character
    /// offset (UTF-16) into span `kobo.N.M`'s text node. Unlike
    /// [`FileIndex::span_start`] this does not land on a visible character —
    /// the caller decides which side of the boundary to resolve toward
    /// (forward for a range start, backward for its exclusive end).
    pub(super) fn span_boundary(&self, n: u32, m: u32, offset_utf16: u32) -> Option<u64> {
        let mark = self.spans.iter().find(|s| s.n == n && s.m == m)?;
        if offset_utf16 == 0 {
            return Some(mark.norm_start);
        }
        // The span's text node is the cluster its first visible character
        // lands in (elements always open a fresh cluster, so text after the
        // span's start tag never coalesces with text before it).
        let cluster = self.clusters.iter().find(|c| {
            c.norm_len > 0
                && mark.norm_start >= c.norm_start
                && mark.norm_start < c.norm_start + c.norm_len
        })?;
        Some(replay_norm(cluster, offset_utf16))
    }

    /// Anchor at the first visible character at/after `t` — the landed
    /// position plus its proving snippet.
    pub(super) fn anchor_at(&self, t: u64) -> Option<Anchor> {
        let landed = self.land(t)?;
        Some(Anchor {
            norm_offset: landed,
            snippet: self.snippet_at(landed),
        })
    }

    /// Anchor at the last visible character strictly before boundary `t` —
    /// the kepub-side counterpart of [`FileIndex::tail_end_at`]'s anchor.
    pub(super) fn back_anchor_before(&self, t: u64) -> Option<Anchor> {
        let landed = self.back_land(t.checked_sub(1)?)?;
        Some(Anchor {
            norm_offset: landed,
            snippet: self.snippet_at(landed),
        })
    }

    /// Kepub walk: which span covers normalized offset `t`? Past the last
    /// span resolves to the last span (end-of-chapter resume).
    pub(super) fn span_at(&self, t: u64) -> Option<(u32, u32)> {
        let mut chosen: Option<&SpanMark> = None;
        for s in &self.spans {
            if s.norm_start <= t {
                chosen = Some(s);
            } else {
                break;
            }
        }
        let mark = chosen.or(self.spans.first())?;
        Some((mark.n, mark.m))
    }

    /// Source walk: the CFI tail addressing normalized offset `t` (landed
    /// on the first visible character at/after it).
    pub(super) fn tail_at(&self, t: u64) -> Option<(CfiTail, Anchor)> {
        let landed = self.land(t)?;
        let cluster = self.clusters.iter().find(|c| {
            c.norm_len > 0 && landed >= c.norm_start && landed < c.norm_start + c.norm_len
        })?;
        let offset_utf16 = replay_offset(cluster, landed)?;
        Some((
            CfiTail {
                element_steps: cluster.element_steps.clone(),
                text_index: cluster.text_index,
                offset_utf16,
            },
            Anchor {
                norm_offset: landed,
                snippet: self.snippet_at(landed),
            },
        ))
    }

    /// Source walk: the normalized offset a CFI tail addresses.
    pub(super) fn offset_at(&self, tail: &CfiTail) -> Option<Anchor> {
        let cluster = self
            .clusters
            .iter()
            .find(|c| c.element_steps == tail.element_steps && c.text_index == tail.text_index)
            .or_else(|| {
                // Element-anchored fallback (synthetic text_index 1 with no
                // matching cluster): first cluster at/under that element.
                self.clusters
                    .iter()
                    .find(|c| c.element_steps.starts_with(&tail.element_steps))
            })?;
        let norm = replay_norm(cluster, tail.offset_utf16);
        let landed = self.land(norm)?;
        Some(Anchor {
            norm_offset: landed,
            snippet: self.snippet_at(landed),
        })
    }

    /// Source walk: the anchor for an *exclusive end* CFI tail — the last
    /// visible character before the boundary the tail addresses, the
    /// reading-direction counterpart of [`FileIndex::offset_at`]. `None`
    /// when nothing visible precedes the boundary (a degenerate range).
    pub(super) fn offset_end_at(&self, tail: &CfiTail) -> Option<Anchor> {
        let cluster = self
            .clusters
            .iter()
            .find(|c| c.element_steps == tail.element_steps && c.text_index == tail.text_index)
            .or_else(|| {
                // Element-anchored fallback, mirroring `offset_at`.
                self.clusters
                    .iter()
                    .find(|c| c.element_steps.starts_with(&tail.element_steps))
            })?;
        let boundary = replay_norm(cluster, tail.offset_utf16);
        let landed = self.back_land(boundary.checked_sub(1)?)?;
        Some(Anchor {
            norm_offset: landed,
            snippet: self.snippet_at(landed),
        })
    }

    /// Kepub walk: the KoboSpan endpoint for the character at normalized
    /// offset `t` — the covering span's counters plus the DOM UTF-16 offset
    /// of that character within the span's text node.
    pub(super) fn span_point_at(&self, t: u64) -> Option<SpanPoint> {
        self.span_point(t, false)
    }

    /// Like [`FileIndex::span_point_at`] but the offset is the exclusive
    /// DOM-Range end *after* the character at `t`.
    pub(super) fn span_point_after(&self, t: u64) -> Option<SpanPoint> {
        self.span_point(t, true)
    }

    /// Shared body of the two span-point queries. Strictly the last span
    /// starting at/before `t` — none of `span_at`'s first/last-span
    /// leniency, which suits a resume position but would anchor an
    /// annotation endpoint to text outside the highlight. `None` when `t`'s
    /// character lives in a different text node than the span's first
    /// visible character: kepubify wraps every text node in its own span,
    /// so that means the two walks disagree about the document.
    fn span_point(&self, t: u64, exclusive_end: bool) -> Option<SpanPoint> {
        let mut chosen: Option<&SpanMark> = None;
        for s in &self.spans {
            if s.norm_start <= t {
                chosen = Some(s);
            } else {
                break;
            }
        }
        let mark = chosen?;
        let cluster = self.clusters.iter().find(|c| {
            c.norm_len > 0
                && mark.norm_start >= c.norm_start
                && mark.norm_start < c.norm_start + c.norm_len
        })?;
        if t < cluster.norm_start || t >= cluster.norm_start + cluster.norm_len {
            return None;
        }
        let char_utf16 = if exclusive_end {
            replay_offset_after(cluster, t)?
        } else {
            replay_offset(cluster, t)?
        };
        Some(SpanPoint {
            n: mark.n,
            m: mark.m,
            char_utf16,
        })
    }

    /// Source walk: the CFI tail addressing the *exclusive end* boundary
    /// after the last visible character strictly before normalized offset
    /// `t` — the DOM-Range end for a highlight whose normalized extent stops
    /// at `t`. Returns the tail plus an anchor at that last character so the
    /// two walks can prove alignment the same way starts do.
    pub(super) fn tail_end_at(&self, t: u64) -> Option<(CfiTail, Anchor)> {
        let last = self.back_land(t.checked_sub(1)?)?;
        let cluster = self
            .clusters
            .iter()
            .find(|c| c.norm_len > 0 && last >= c.norm_start && last < c.norm_start + c.norm_len)?;
        let offset_utf16 = replay_offset_after(cluster, last)?;
        Some((
            CfiTail {
                element_steps: cluster.element_steps.clone(),
                text_index: cluster.text_index,
                offset_utf16,
            },
            Anchor {
                norm_offset: last,
                snippet: self.snippet_at(last),
            },
        ))
    }

    /// Last visible (non-space) normalized offset at/before `t`. `None` when
    /// nothing visible precedes it.
    fn back_land(&self, t: u64) -> Option<u64> {
        self.norm
            .chars()
            .take(usize::try_from(t).map_or(usize::MAX, |t| t.saturating_add(1)))
            .enumerate()
            .filter(|(_, c)| *c != ' ')
            .last()
            .map(|(i, _)| i as u64)
    }

    /// First visible (non-space) normalized offset at/after `t`. An offset
    /// at/past the end of text resolves to the last character (so an
    /// end-of-file position still lands somewhere); `None` only for a
    /// document with no text at all.
    fn land(&self, t: u64) -> Option<u64> {
        let mut idx = t;
        let mut chars = self
            .norm
            .chars()
            .skip(usize::try_from(t).unwrap_or(usize::MAX));
        loop {
            match chars.next() {
                Some(' ') => idx += 1,
                Some(_) => return Some(idx),
                None => {
                    // Past the end: land on the last visible char so an
                    // end-of-file position still resolves.
                    let total = self.norm.chars().count() as u64;
                    return total.checked_sub(1);
                }
            }
        }
    }
}

/// `char::len_utf16` as `u32` — the length is always 1 or 2.
fn len_utf16_u32(c: char) -> u32 {
    u32::try_from(c.len_utf16()).unwrap_or(2)
}

/// Replay whitespace collapsing inside one cluster to convert a normalized
/// offset back to a raw UTF-16 offset in the cluster's DOM text node.
fn replay_offset(cluster: &Cluster, norm_target: u64) -> Option<u32> {
    let mut norm_pos = cluster.norm_start;
    let mut utf16: u32 = 0;
    let mut in_run = cluster.entered_in_run;
    for c in cluster.raw.chars() {
        if c.is_whitespace() {
            if !in_run {
                if norm_pos == norm_target {
                    // Target is the emitted space — never happens after
                    // `land`, which skips spaces, but map it anyway.
                    return Some(utf16);
                }
                norm_pos += 1;
            }
            in_run = true;
        } else {
            if norm_pos == norm_target {
                return Some(utf16);
            }
            norm_pos += 1;
            in_run = false;
        }
        utf16 += len_utf16_u32(c);
    }
    None
}

/// Like [`replay_offset`] but returns the raw UTF-16 offset *after* the
/// character at `norm_target` — the exclusive DOM-Range end that includes it.
fn replay_offset_after(cluster: &Cluster, norm_target: u64) -> Option<u32> {
    let mut norm_pos = cluster.norm_start;
    let mut utf16: u32 = 0;
    let mut in_run = cluster.entered_in_run;
    for c in cluster.raw.chars() {
        let emits = if c.is_whitespace() {
            let emits = !in_run;
            in_run = true;
            emits
        } else {
            in_run = false;
            true
        };
        utf16 += len_utf16_u32(c);
        if emits {
            if norm_pos == norm_target {
                return Some(utf16);
            }
            norm_pos += 1;
        }
    }
    None
}

/// Inverse of [`replay_offset`]: the normalized offset for a raw UTF-16
/// offset into the cluster. Offsets inside a whitespace run or past the
/// cluster's end saturate to the following normalized position.
fn replay_norm(cluster: &Cluster, utf16_target: u32) -> u64 {
    let mut norm_pos = cluster.norm_start;
    let mut utf16: u32 = 0;
    let mut in_run = cluster.entered_in_run;
    for c in cluster.raw.chars() {
        if utf16 >= utf16_target {
            return norm_pos;
        }
        if c.is_whitespace() {
            if !in_run {
                norm_pos += 1;
            }
            in_run = true;
        } else {
            norm_pos += 1;
            in_run = false;
        }
        utf16 += len_utf16_u32(c);
    }
    norm_pos
}
