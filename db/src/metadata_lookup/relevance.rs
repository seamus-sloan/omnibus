//! Scoring, filtering, and ordering a provider's candidates against the query
//! that asked for them.
//!
//! A provider ranks for its own purposes: ask any catalog for a famous novel
//! and it will hand back study guides, box sets, and "the notebooks of"
//! alongside the book. Ordering alone can't fix that — the study guide is
//! still on screen, just lower — so candidates are scored here and the ones
//! that are coincidences rather than matches are dropped before they are
//! returned.
//!
//! Ported from BookOrbit's `candidate-relevance.ts` + `title-match.ts`, whose
//! constants and thresholds are reproduced exactly; the two deliberate
//! divergences are called out on [`normalize_title_text`] and [`levenshtein`].

use std::sync::OnceLock;

use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::ProviderEdition;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use super::query::SearchQuery;

/// An exact ISBN match. Far above anything a title can score, because it is
/// the one signal that identifies an edition rather than resembling one.
pub const ISBN_MATCH_SCORE: f64 = 100.0;
/// A candidate author that contains, or is contained by, the query's.
pub const AUTHOR_MATCH_SCORE: f64 = 3.0;
/// A candidate author merely sharing one significant word with the query's.
pub const AUTHOR_TOKEN_MATCH_SCORE: f64 = 1.0;

/// The weakest title signal a candidate may rest on. Below this it is a
/// coincidence — a shared word or two, or two unrelated titles that happen to
/// be similar in length. Low enough that a typo'd title still resolves
/// through the fuzzy path.
pub const MIN_RELEVANCE_SCORE: f64 = 3.0;

/// Title-tier scores. The first three are exclusive and return immediately;
/// the last two are computed together and the better one wins.
const EXACT: f64 = 10.0;
const PREFIX: f64 = 8.0;
const SUBSTRING: f64 = 7.0;
const TOKEN_OVERLAP: f64 = 6.0;
const LEVENSHTEIN: f64 = 4.0;

/// Minimum edit-distance similarity for the fuzzy tier — **inclusive**.
const MIN_LEVENSHTEIN_SIMILARITY: f64 = 0.6;
/// Minimum share of the query's significant words the candidate must carry —
/// **exclusive**. A minority of the query's meaningful words in common is not
/// evidence of the same book: "The Way of Kings" and "The Way We Were" share
/// exactly one.
const MIN_TOKEN_OVERLAP_RATIO: f64 = 0.5;

/// Words that appear in so many titles that requiring them ranks nothing.
/// Listed rather than inferred from length: "The" and "Ida" are both three
/// letters and only one of them is noise.
///
/// Applied to the **query side only**. A stopword left in a candidate can no
/// longer match anything once the query side is stripped, so filtering both
/// would only shrink the numerator.
const STOPWORDS: &[&str] = &[
    "the", "an", "and", "or", "of", "in", "on", "at", "to", "for", "with", "from", "by", "is",
    "it", "as", "be", "are", "was", "were", "this", "that", "these", "those", "into", "over",
    "under", "its", "his", "her", "their", "our", "your", "my",
];

/// Titles that name a book *about* a book. These are not low-ranking matches
/// to be sorted downward — they are a different work, and a picker offering
/// to overwrite a novel's metadata with a study guide's is offering a mistake.
fn skip_patterns() -> &'static [Regex; 2] {
    static PATTERNS: OnceLock<[Regex; 2]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            Regex::new(
                r"(?i)^(summary|study guide|analysis|guide to|workbook|review of|summary of|chapter summary|chapter-by-chapter)\b",
            )
            .expect("skip pattern is a compile-time literal"),
            Regex::new(
                r"(?i)\b(summary|study guide|book analysis|reading guide|literature guide|workbook|digest|breakdown and analysis)\b",
            )
            .expect("skip pattern is a compile-time literal"),
        ]
    })
}

/// Combining marks that carry no identity: NFKD splits `ō` into `o` + a
/// combining macron, and dropping the macron is what makes "Sōseki" and
/// "Soseki" the same book.
fn nonspacing_marks() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\p{Mn}+").expect("compile-time literal"))
}

/// Everything that isn't a letter, a digit, or a **spacing** combining mark.
fn non_word() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^\p{L}\p{N}\p{Mc}]+").expect("compile-time literal"))
}

/// Fold a title to its comparable form: decomposed, stripped of nonspacing
/// marks, lowercased, and reduced to letters and digits of any script.
///
/// Restricting this to `[a-z0-9]` would empty every title written in a
/// non-Latin script, making two providers reporting the identical Japanese or
/// Russian title look like different books.
///
/// **Deliberate divergence from the source implementation**: spacing combining
/// marks (`\p{Mc}`) are *kept*. BookOrbit's comment says they are, but its
/// character class drops them — which splits every Devanagari, Bengali, or
/// Tamil word at its vowel signs ("हिन्दी" → "ह नद") and then loses most of the
/// fragments to the one-character token filter. We keep them, as that comment
/// intended.
pub fn normalize_title_text(value: &str) -> String {
    let decomposed: String = value.nfkd().collect();
    let stripped = nonspacing_marks().replace_all(&decomposed, "");
    // Whole-string lowercase, not per-char: only this form applies the Greek
    // final-sigma rule, so "ΟΔΥΣΣΕΥΣ" and "Οδυσσευς" agree.
    let lowered = stripped.to_lowercase();
    non_word().replace_all(&lowered, " ").trim().to_string()
}

/// Split a normalized title into tokens, dropping single-character ones —
/// initials, stray digits, and the "a" in "a tale of 2 cities" carry no
/// identifying signal.
pub fn tokenize(normalized: &str) -> Vec<&str> {
    normalized
        .split(' ')
        .filter(|t| t.chars().count() > 1)
        .collect()
}

/// The query's tokens minus stopwords — falling back to the raw tokens when
/// that would leave nothing, so a book actually titled "It" stays matchable.
fn significant<'a>(tokens: &[&'a str]) -> Vec<&'a str> {
    let kept: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|t| !STOPWORDS.contains(t))
        .collect();
    if kept.is_empty() {
        tokens.to_vec()
    } else {
        kept
    }
}

/// Whether `needle` appears in `haystack` on whole-word boundaries. After
/// normalization the only boundaries left are the single space and the ends.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    // Advanced one byte at a time rather than by `match_indices`, which yields
    // only *disjoint* matches: for "won on on" / "on on" the boundary-aligned
    // occurrence overlaps an earlier one and would never be examined.
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let at = from + offset;
        if at_boundary(haystack, at, needle.len()) {
            return true;
        }
        // Next char boundary, so the slice index stays valid on any input.
        from = at + haystack[at..].chars().next().map_or(1, char::len_utf8);
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// Whether `haystack` begins with `needle` on a whole-word boundary.
fn starts_with_word(haystack: &str, needle: &str) -> bool {
    !needle.is_empty() && haystack.starts_with(needle) && at_boundary(haystack, 0, needle.len())
}

/// Whether the slice `[at, at + len)` is delimited by spaces or string ends.
fn at_boundary(haystack: &str, at: usize, len: usize) -> bool {
    let before = at == 0 || haystack.as_bytes()[at - 1] == b' ';
    let end = at + len;
    let after = end == haystack.len() || haystack.as_bytes()[end] == b' ';
    before && after
}

/// Plain Levenshtein edit distance over Unicode scalar values.
///
/// **Not** Damerau — a transposition costs two edits, not one — matching the
/// `fastest-levenshtein` semantics this port reproduces. Counted in `char`s
/// rather than the source's UTF-16 code units, which agree for everything in
/// the Basic Multilingual Plane and differ only for astral-plane text, where
/// counting scalar values is the more defensible answer anyway.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Edit-distance similarity in `0.0..=1.0`, normalized by the **longer** side.
fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let max = ac.len().max(bc.len());
    if max == 0 {
        return 1.0;
    }
    1.0 - (levenshtein(&ac, &bc) as f64 / max as f64)
}

/// How strongly two titles refer to the same work.
///
/// Five tiers. The first three are exact-ish and return immediately; when all
/// three miss, the token-overlap and fuzzy paths are both computed and the
/// better one wins. Argument order matters only to the token tier, whose
/// ratio is measured against the *query's* words.
pub fn score_title_match(query: &str, candidate: &str) -> f64 {
    let q = normalize_title_text(query);
    let c = normalize_title_text(candidate);
    if q.is_empty() || c.is_empty() {
        return 0.0;
    }
    if q == c {
        return EXACT;
    }
    if starts_with_word(&c, &q) || starts_with_word(&q, &c) {
        return PREFIX;
    }
    if contains_word(&c, &q) || contains_word(&q, &c) {
        return SUBSTRING;
    }

    let query_tokens = significant(&tokenize(&q));
    let candidate_tokens: std::collections::HashSet<&str> = tokenize(&c).into_iter().collect();
    let ratio = if query_tokens.is_empty() {
        0.0
    } else {
        let hit = query_tokens
            .iter()
            .filter(|t| candidate_tokens.contains(*t))
            .count();
        hit as f64 / query_tokens.len() as f64
    };
    // Strictly greater, where the fuzzy tier below is inclusive. The
    // asymmetry is load-bearing: a ratio of exactly one half is the
    // "The Way of Kings" / "The Way We Were" case, and must not score.
    let token_score = if ratio > MIN_TOKEN_OVERLAP_RATIO {
        ratio * TOKEN_OVERLAP
    } else {
        0.0
    };

    let similarity = levenshtein_similarity(&q, &c);
    let fuzzy_score = if similarity >= MIN_LEVENSHTEIN_SIMILARITY {
        similarity * LEVENSHTEIN
    } else {
        0.0
    };

    token_score.max(fuzzy_score)
}

/// Whether any of `authors` matches the query's author.
///
/// Containment either way, because providers disagree on how much of a name
/// to carry ("Tolkien" vs "J. R. R. Tolkien"); a shared significant word is
/// the weaker fallback.
fn score_author(query: &str, authors: &[String]) -> f64 {
    let q = normalize_title_text(query);
    if q.is_empty() {
        return 0.0;
    }
    for author in authors {
        let a = normalize_title_text(author);
        if a.is_empty() {
            continue;
        }
        if a.contains(&q) || q.contains(&a) {
            return AUTHOR_MATCH_SCORE;
        }
    }
    // A *name* token, not a title token: no stopword removal (a stopword in a
    // name is part of the name), and a longer minimum, because two-letter
    // fragments of names collide constantly. This deliberately differs from
    // the title tiers' `significant`/`tokenize` pair, and matches the source
    // implementation's own `shareSignificantToken`.
    let q_tokens = name_tokens(&q);
    let shared = authors.iter().any(|author| {
        let a = normalize_title_text(author);
        let a_tokens: std::collections::HashSet<String> = name_tokens(&a).into_iter().collect();
        q_tokens.iter().any(|t| a_tokens.contains(t))
    });
    if shared {
        AUTHOR_TOKEN_MATCH_SCORE
    } else {
        0.0
    }
}

/// The words of a name worth matching on: more than two characters, and
/// stopwords kept.
fn name_tokens(normalized: &str) -> Vec<String> {
    normalized
        .split(' ')
        .filter(|t| t.chars().count() > 2)
        .map(str::to_string)
        .collect()
}

/// Whether `edition` carries `isbn13` on either of its identifiers.
fn matches_isbn(edition: &ProviderEdition, isbn13: &str) -> bool {
    let same = |v: &Option<String>| {
        v.as_deref()
            .and_then(|raw| normalize_isbn(raw).ok())
            .is_some_and(|norm| norm == isbn13)
    };
    same(&edition.isbn13) || same(&edition.isbn10)
}

/// Score one candidate against the query.
///
/// An exact ISBN wins outright and short-circuits — it identifies the edition,
/// so nothing a title says can improve or spoil it. Otherwise the title
/// carries the score and the author only *adds* to it: a matching author with
/// no title signal is a different book by the same person, which is precisely
/// what the picker must not offer.
pub fn score_candidate(edition: &ProviderEdition, query: &SearchQuery) -> f64 {
    if let Some(isbn13) = query.isbn13.as_deref() {
        if matches_isbn(edition, isbn13) {
            return ISBN_MATCH_SCORE;
        }
    }
    let mut score = 0.0;
    if let Some(title) = query.title.as_deref() {
        score += score_title_match(title, &edition.title);
    }
    if score > 0.0 {
        if let Some(author) = query.author.as_deref() {
            if !edition.authors.is_empty() {
                score += score_author(author, &edition.authors);
            }
        }
    }
    score
}

/// Drop the coincidences, order what is left, and cap it.
///
/// Returns candidates unchanged (bar the cap) when the query carries neither
/// a title nor an author: there is nothing to score against, and inventing a
/// ranking from a bare ISBN query would only shuffle rows arbitrarily.
///
/// Each survivor is stamped with its own score, so the client can order by a
/// number derived from the candidate and the query — never from which
/// provider answered.
pub fn filter_and_rank(
    editions: Vec<ProviderEdition>,
    query: &SearchQuery,
    limit: usize,
) -> Vec<ProviderEdition> {
    // Without a title there is nothing to rank against: the author bonus only
    // applies on top of a title signal, so scoring here would give every
    // candidate zero and silently discard the lot. Returning them unscored is
    // the honest answer — the caller sees what the provider found.
    if query.title.is_none() {
        return editions.into_iter().take(limit).collect();
    }

    let mut scored: Vec<(f64, ProviderEdition)> = editions
        .into_iter()
        .map(|e| (score_candidate(&e, query), e))
        // The skip-list is a heuristic about *titles*; an exact ISBN is a fact
        // about the edition, so it wins. Without this, a reader editing a book
        // that genuinely is a study guide — the query is seeded from the book
        // itself — could never find it, not even by its own ISBN.
        .filter(|(score, e)| *score >= ISBN_MATCH_SCORE || !is_about_a_book(&e.title))
        .filter(|(score, _)| *score >= MIN_RELEVANCE_SCORE)
        .collect();

    // Stable, so candidates that scored identically keep the order their
    // provider returned them in rather than being shuffled.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(limit)
        .map(|(score, mut e)| {
            e.relevance = Some(to_fixed_point(score));
            e
        })
        .collect()
}

/// Whether this title names a book *about* a book. Matched against the raw
/// title, not the normalized one, since the patterns rely on punctuation and
/// word boundaries normalization removes.
fn is_about_a_book(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && skip_patterns().iter().any(|p| p.is_match(trimmed))
}

/// Score to the hundredths the wire carries, saturating rather than wrapping.
fn to_fixed_point(score: f64) -> i32 {
    (score * 100.0)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

#[cfg(test)]
mod tests;
