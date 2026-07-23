//! Small display formatters shared across surfaces that render `book_files`
//! — the hero's file picker and the admin delete dialog. Pure functions, no
//! Dioxus, so they unit-test without a renderer.

use omnibus_shared::BookFileInfo;

/// Human-readable file size (`"3.1 MB"`), or `None` when the row carries no
/// usable size — an unstat'd or zero-byte file shows no size rather than
/// `"0 B"`.
pub fn file_size(size_bytes: i64) -> Option<String> {
    let bytes = u64::try_from(size_bytes).ok().filter(|bytes| *bytes > 0)?;
    let (value, unit) = if bytes >= 1_000_000_000 {
        (bytes as f64 / 1_000_000_000.0, "GB")
    } else if bytes >= 1_000_000 {
        (bytes as f64 / 1_000_000.0, "MB")
    } else if bytes >= 1_000 {
        (bytes as f64 / 1_000.0, "KB")
    } else {
        return Some(format!("{bytes} B"));
    };
    Some(format!("{value:.1} {unit}"))
}

/// `"EPUB · Part 2"` — the format plus the file's own label, falling back to
/// its 1-based ordinal when it has none.
pub fn file_label(file: &BookFileInfo) -> String {
    let label = file
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Part {}", file.ordinal + 1));
    format!("{} \u{b7} {label}", file.format.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(format: &str, ordinal: i64, label: Option<&str>) -> BookFileInfo {
        BookFileInfo {
            id: 1,
            format: format.to_string(),
            filename: "f".into(),
            ordinal,
            label: label.map(str::to_string),
            size_bytes: 0,
            path: None,
        }
    }

    #[test]
    fn file_size_scales_the_unit_to_the_byte_count() {
        assert_eq!(file_size(512), Some("512 B".into()));
        assert_eq!(file_size(3_100), Some("3.1 KB".into()));
        assert_eq!(file_size(3_100_000), Some("3.1 MB".into()));
        assert_eq!(file_size(2_500_000_000), Some("2.5 GB".into()));
    }

    #[test]
    fn file_size_is_absent_for_an_unstated_size() {
        assert_eq!(file_size(0), None);
        assert_eq!(file_size(-1), None);
    }

    #[test]
    fn file_label_prefers_the_stored_label_over_the_ordinal() {
        assert_eq!(
            file_label(&file("epub", 0, Some("10th anniversary"))),
            "EPUB · 10th anniversary"
        );
    }

    #[test]
    fn file_label_falls_back_to_a_one_based_part_number() {
        assert_eq!(file_label(&file("mp3", 1, None)), "MP3 · Part 2");
        assert_eq!(file_label(&file("mp3", 1, Some("  "))), "MP3 · Part 2");
    }
}
