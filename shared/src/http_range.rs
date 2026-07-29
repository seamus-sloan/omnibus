//! Pure single-range `Range` header parser.
//!
//! Shared by the server's byte-serving endpoints and the mobile loopback
//! media proxy, so a range means the same thing on both sides of a
//! downloaded book. `<audio>`, epub.js and the download engine only ever
//! send a single `bytes=` range, so multi-range requests are deliberately
//! ignored (a server MAY ignore a Range header — the client just gets a 200
//! with the full body).

/// How a server should answer a request against a `len`-byte file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOutcome {
    /// No (usable) range — respond 200 with the full body.
    Full,
    /// Respond 206 with bytes `start..=end` (inclusive, both < len).
    Partial { start: u64, end: u64 },
    /// Respond 416 with `Content-Range: bytes */{len}`.
    NotSatisfiable,
}

/// Parse an optional `Range` header value against a file of `len` bytes.
///
/// Handles the three single-range forms (`bytes=a-b`, `bytes=a-`,
/// `bytes=-suffix`). Malformed or multi-range headers fall back to
/// [`RangeOutcome::Full`] per RFC 9110 (a server may ignore Range);
/// syntactically valid but unsatisfiable ranges yield
/// [`RangeOutcome::NotSatisfiable`].
pub fn parse(header: Option<&str>, len: u64) -> RangeOutcome {
    let Some(raw) = header else {
        return RangeOutcome::Full;
    };
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return RangeOutcome::Full;
    };
    if spec.contains(',') {
        return RangeOutcome::Full;
    }
    let Some((start_s, end_s)) = spec.split_once('-') else {
        return RangeOutcome::Full;
    };
    let (start_s, end_s) = (start_s.trim(), end_s.trim());

    if start_s.is_empty() {
        // Suffix form: last N bytes.
        let Ok(suffix) = end_s.parse::<u64>() else {
            return RangeOutcome::Full;
        };
        if suffix == 0 || len == 0 {
            return RangeOutcome::NotSatisfiable;
        }
        let start = len.saturating_sub(suffix);
        return RangeOutcome::Partial {
            start,
            end: len - 1,
        };
    }

    let Ok(start) = start_s.parse::<u64>() else {
        return RangeOutcome::Full;
    };
    if start >= len {
        return RangeOutcome::NotSatisfiable;
    }
    let end = if end_s.is_empty() {
        len - 1
    } else {
        match end_s.parse::<u64>() {
            // An end past EOF clamps to the last byte (RFC 9110 §14.1.2).
            Ok(e) => e.min(len - 1),
            Err(_) => return RangeOutcome::Full,
        }
    };
    if end < start {
        return RangeOutcome::Full;
    }
    RangeOutcome::Partial { start, end }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_full_when_header_absent_or_not_bytes() {
        assert_eq!(parse(None, 100), RangeOutcome::Full);
        assert_eq!(parse(Some("items=0-5"), 100), RangeOutcome::Full);
    }

    #[test]
    fn parse_handles_bounded_range() {
        assert_eq!(
            parse(Some("bytes=0-499"), 1000),
            RangeOutcome::Partial { start: 0, end: 499 }
        );
        assert_eq!(
            parse(Some("bytes=500-999"), 1000),
            RangeOutcome::Partial {
                start: 500,
                end: 999
            }
        );
    }

    #[test]
    fn parse_handles_open_ended_range() {
        assert_eq!(
            parse(Some("bytes=500-"), 1000),
            RangeOutcome::Partial {
                start: 500,
                end: 999
            }
        );
    }

    #[test]
    fn parse_handles_suffix_range() {
        assert_eq!(
            parse(Some("bytes=-200"), 1000),
            RangeOutcome::Partial {
                start: 800,
                end: 999
            }
        );
        // Suffix longer than the file clamps to the whole file.
        assert_eq!(
            parse(Some("bytes=-5000"), 1000),
            RangeOutcome::Partial { start: 0, end: 999 }
        );
    }

    #[test]
    fn parse_clamps_end_past_eof() {
        assert_eq!(
            parse(Some("bytes=900-4000"), 1000),
            RangeOutcome::Partial {
                start: 900,
                end: 999
            }
        );
    }

    #[test]
    fn parse_flags_unsatisfiable_ranges() {
        assert_eq!(
            parse(Some("bytes=1000-"), 1000),
            RangeOutcome::NotSatisfiable
        );
        assert_eq!(parse(Some("bytes=-0"), 1000), RangeOutcome::NotSatisfiable);
        assert_eq!(parse(Some("bytes=0-"), 0), RangeOutcome::NotSatisfiable);
    }

    #[test]
    fn parse_ignores_garbage_and_multi_range() {
        assert_eq!(parse(Some("bytes=abc-def"), 100), RangeOutcome::Full);
        assert_eq!(parse(Some("bytes=5-2"), 100), RangeOutcome::Full);
        assert_eq!(parse(Some("bytes=0-1,5-9"), 100), RangeOutcome::Full);
    }
}
