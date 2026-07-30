//! Parse and format the Kobo `CurrentBookmark.Location` payload — the
//! `{"Source", "Type": "KoboSpan", "Value": "kobo.N.M"}` object a device
//! reports as its position. Only `KoboSpan` locations are convertible; any
//! other `Type` is opaque to us and parses to `None`.

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
