//! Wire types for the "fetch book summary" action: which external source a
//! fetch targets, and the word-count gate the book-detail page uses to decide
//! whether to surface the button.

use serde::{Deserialize, Serialize};

/// Which external provider a summary fetch should hit. The client drives the
/// cascade (Hardcover first when a key exists, else OpenLibrary) by calling the
/// fetch endpoint once per source, so it can show a per-source status message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummarySource {
    Hardcover,
    OpenLibrary,
}

/// Below this many words, the current summary counts as "sparse" and the
/// book-detail page offers the fetch button.
pub const SUMMARY_MIN_WORDS: usize = 10;

/// Count whitespace-separated words in a summary, ignoring any HTML tags (the
/// stored description may contain markup, which shouldn't inflate the count).
pub fn summary_word_count(summary: &str) -> usize {
    strip_tags(summary).split_whitespace().count()
}

/// `true` when `summary` has fewer than [`SUMMARY_MIN_WORDS`] words — the
/// condition under which the book-detail page shows the fetch button.
pub fn summary_is_sparse(summary: &str) -> bool {
    summary_word_count(summary) < SUMMARY_MIN_WORDS
}

/// Replace anything between `<` and `>` with a single space so tag
/// names/attributes aren't counted as words *and* adjacent text isn't merged
/// (`hello<br>there` → two words, not one). Deliberately simple — a word count
/// doesn't need a real HTML parser; `split_whitespace` collapses the extra
/// spaces this introduces.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_word_count_counts_plain_words() {
        assert_eq!(summary_word_count("one two three"), 3);
        assert_eq!(summary_word_count("   spaced   out  words "), 3);
        assert_eq!(summary_word_count(""), 0);
    }

    #[test]
    fn summary_word_count_ignores_html_tags() {
        assert_eq!(summary_word_count("<p>hello <b>there</b> friend</p>"), 3);
        assert_eq!(summary_word_count("<div class=\"x\">only one</div>"), 2);
    }

    #[test]
    fn summary_word_count_does_not_merge_words_across_tags() {
        // A tag with no surrounding whitespace must still separate the words it
        // sits between, so the count isn't undercounted (regression guard).
        assert_eq!(summary_word_count("hello<br>there"), 2);
        assert_eq!(summary_word_count("one<span>two</span>three"), 3);
    }

    #[test]
    fn summary_is_sparse_uses_ten_word_threshold() {
        // Nine words → sparse.
        assert!(summary_is_sparse("a b c d e f g h i"));
        // Ten words → not sparse (boundary).
        assert!(!summary_is_sparse("a b c d e f g h i j"));
    }
}
