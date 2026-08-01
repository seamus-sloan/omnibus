//! Parse and format the two Kobo location payloads: the
//! `CurrentBookmark.Location` object (`{"Source", "Type": "KoboSpan",
//! "Value": "kobo.N.M"}`) a device reports as its position, and the
//! annotation `location.span` range it anchors highlights with. Only
//! KoboSpan-shaped anchors are convertible; anything else parses to `None`.

/// A parsed KoboSpan bookmark: the spine document (`Source`, an OPF-relative
/// href) plus kepubify's paragraph/segment counters from `Value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KoboLoc {
    pub source: String,
    pub n: u32,
    pub m: u32,
}

/// Parse a stored `Location` JSON string. `None` unless `Type` is
/// `KoboSpan` and `Value` matches `kobo.<N>.<M>`.
pub fn parse_location(raw_json: &str) -> Option<KoboLoc> {
    let v: serde_json::Value = serde_json::from_str(raw_json).ok()?;
    if v.get("Type")?.as_str()? != "KoboSpan" {
        return None;
    }
    let source = v.get("Source")?.as_str()?.to_owned();
    if source.is_empty() {
        return None;
    }
    let (n, m) = parse_span_id(v.get("Value")?.as_str()?)?;
    Some(KoboLoc { source, n, m })
}

/// Format a `Location` JSON string for the given source href and counters.
pub fn location_json(source: &str, n: u32, m: u32) -> String {
    serde_json::json!({
        "Source": source,
        "Type": "KoboSpan",
        "Value": span_id(n, m),
    })
    .to_string()
}

/// The DOM id kepubify assigns the `(n, m)` segment: `kobo.N.M`.
pub fn span_id(n: u32, m: u32) -> String {
    format!("kobo.{n}.{m}")
}

/// One endpoint of an annotation anchor: a KoboSpan plus a DOM character
/// offset (UTF-16 code units) into that span's text node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanPoint {
    pub n: u32,
    pub m: u32,
    pub char_utf16: u32,
}

/// A parsed annotation anchor: the spine document plus the start/end
/// endpoints of the highlighted range. `end` is exclusive, DOM-Range style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KoboSpanRange {
    pub source: String,
    pub start: SpanPoint,
    pub end: SpanPoint,
}

/// Parse a stored annotation `location` JSON string. `None` unless the
/// `span` object carries a chapter filename and both endpoint paths name
/// `kobo.N.M` spans.
pub fn parse_annotation_location(raw_json: &str) -> Option<KoboSpanRange> {
    let v: serde_json::Value = serde_json::from_str(raw_json).ok()?;
    let span = v.get("span")?;
    let source = span.get("chapterFilename")?.as_str()?;
    if source.is_empty() {
        return None;
    }
    let point = |path_key: &str, char_key: &str| -> Option<SpanPoint> {
        let (n, m) = parse_span_path(span.get(path_key)?.as_str()?)?;
        let char_utf16 = span
            .get(char_key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|c| u32::try_from(c).ok())
            .unwrap_or(0);
        Some(SpanPoint { n, m, char_utf16 })
    };
    Some(KoboSpanRange {
        source: source.to_owned(),
        start: point("startPath", "startChar")?,
        end: point("endPath", "endChar")?,
    })
}

/// Format an annotation `location` JSON string for a derived span range —
/// the exact `span` shape a device's own uploads carry (and
/// [`parse_annotation_location`] reads back), so a converted web highlight
/// is indistinguishable from a native one on the wire.
pub fn annotation_location_json(source: &str, start: SpanPoint, end: SpanPoint) -> String {
    serde_json::json!({
        "span": {
            "chapterFilename": source,
            "startPath": span_path(start.n, start.m),
            "startChar": start.char_utf16,
            "endPath": span_path(end.n, end.m),
            "endChar": end.char_utf16,
        }
    })
    .to_string()
}

/// The CSS-ish selector an annotation endpoint path carries:
/// `span#kobo\.N\.M` (dots backslash-escaped).
fn span_path(n: u32, m: u32) -> String {
    format!("span#kobo\\.{n}\\.{m}")
}

/// Extract the `(n, m)` counters from an annotation endpoint path — a
/// CSS-ish selector like `span#kobo\.55\.1` (dots backslash-escaped).
fn parse_span_path(path: &str) -> Option<(u32, u32)> {
    let id = path.rsplit('#').next()?;
    parse_span_id(&id.replace('\\', ""))
}

/// Parse a `kobo.<N>.<M>` span id into its counters.
pub fn parse_span_id(value: &str) -> Option<(u32, u32)> {
    let rest = value.strip_prefix("kobo.")?;
    let (n, m) = rest.split_once('.')?;
    // `parse::<u32>` alone would accept `+1`/leading zeros oddities we never
    // emit; requiring all-digits keeps the accepted grammar exactly kobo.N.M.
    if n.is_empty() || m.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !m.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((n.parse().ok()?, m.parse().ok()?))
}
