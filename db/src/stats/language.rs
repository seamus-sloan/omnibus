//! One bucket per language for the composition breakdown.
//!
//! `languages.code` is whatever the file declared, and files disagree: an
//! EPUB may say `en`, a Calibre export `eng`, a US publisher `en-US`. Left
//! alone those are three slices of a 28-book library that reads as three
//! languages (#2466). This folds every spelling of one language onto its
//! English name, so the breakdown counts languages rather than spellings.

/// The primary language subtag of a BCP-47 tag, lowercased: everything before
/// the first `-`/`_`, so `en-US`, `pt_BR` and `zh-Hant` reduce to `en`, `pt`
/// and `zh`. Region and script are deliberately dropped — a language
/// breakdown that separates Brazilian from European Portuguese is answering a
/// question nobody asked of a library's shelf.
fn primary_subtag(code: &str) -> String {
    code.trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// ISO 639-2/B and /T three-letter codes onto their two-letter 639-1 form.
/// Only the pairs that actually differ, plus the bibliographic variants that
/// differ from the terminological one (`ger`/`deu`, `fre`/`fra`, ...), which
/// is where the duplicate buckets come from in practice.
const ALIASES: &[(&str, &str)] = &[
    ("ara", "ar"),
    ("ben", "bn"),
    ("bul", "bg"),
    ("cat", "ca"),
    ("ces", "cs"),
    ("cze", "cs"),
    ("dan", "da"),
    ("deu", "de"),
    ("dut", "nl"),
    ("ell", "el"),
    ("eng", "en"),
    ("epo", "eo"),
    ("est", "et"),
    ("fas", "fa"),
    ("fin", "fi"),
    ("fra", "fr"),
    ("fre", "fr"),
    ("ger", "de"),
    ("gle", "ga"),
    ("gre", "el"),
    ("heb", "he"),
    ("hin", "hi"),
    ("hrv", "hr"),
    ("hun", "hu"),
    ("ice", "is"),
    ("ind", "id"),
    ("isl", "is"),
    ("ita", "it"),
    ("jpn", "ja"),
    ("kor", "ko"),
    ("lat", "la"),
    ("lav", "lv"),
    ("lit", "lt"),
    ("may", "ms"),
    ("msa", "ms"),
    ("nld", "nl"),
    ("nor", "no"),
    ("per", "fa"),
    ("pol", "pl"),
    ("por", "pt"),
    ("ron", "ro"),
    ("rum", "ro"),
    ("rus", "ru"),
    ("slk", "sk"),
    ("slo", "sk"),
    ("slv", "sl"),
    ("spa", "es"),
    ("srp", "sr"),
    ("swe", "sv"),
    ("tha", "th"),
    ("tur", "tr"),
    ("ukr", "uk"),
    ("urd", "ur"),
    ("vie", "vi"),
    ("zho", "zh"),
    ("chi", "zh"),
];

/// Two-letter codes onto the English name the breakdown renders. Covers the
/// languages a self-hosted library plausibly holds; anything outside it keeps
/// its own code rather than being folded into a bucket this can't name.
const NAMES: &[(&str, &str)] = &[
    ("ar", "Arabic"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("eo", "Esperanto"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("ga", "Irish"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("la", "Latin"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("ms", "Malay"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sr", "Serbian"),
    ("sv", "Swedish"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("vi", "Vietnamese"),
    ("zh", "Chinese"),
];

/// Label of the bucket a declared-but-meaningless code lands in.
pub(super) const UNKNOWN_LABEL: &str = "Unknown";

/// The display label one `languages.code` belongs under.
///
/// `und` (undetermined), `mul` (multiple), `zxx` (no linguistic content) and
/// an empty code are all [`UNKNOWN_LABEL`]: they are a file declining to
/// answer, and four spellings of that are still one bucket. A code with no
/// name here keeps its own primary subtag, uppercased — better an honest `QU`
/// than a wrong name or a silent fold into "Other".
pub(super) fn language_label(code: &str) -> String {
    let primary = primary_subtag(code);
    if matches!(primary.as_str(), "" | "und" | "mul" | "zxx" | "mis") {
        return UNKNOWN_LABEL.to_string();
    }
    let two = ALIASES
        .iter()
        .find(|(from, _)| *from == primary)
        .map_or(primary.as_str(), |(_, to)| *to);
    NAMES
        .iter()
        .find(|(code, _)| *code == two)
        .map_or_else(|| two.to_ascii_uppercase(), |(_, name)| (*name).to_string())
}

#[cfg(test)]
mod tests;
