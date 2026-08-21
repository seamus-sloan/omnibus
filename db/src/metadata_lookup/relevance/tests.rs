//! Tests for the relevance port. The tiering, the two thresholds, and the
//! Unicode folding are all places a plausible-looking implementation diverges
//! silently, so each is pinned by a case rather than by inspection.

use omnibus_shared::metadata_lookup::{MetadataProvider, ProviderEdition};

use super::*;

/// A candidate with just enough on it to be scored.
fn candidate(title: &str, authors: &[&str]) -> ProviderEdition {
    ProviderEdition {
        source: MetadataProvider::OpenLibrary,
        provider_ref: format!("/works/{title}"),
        isbn13: None,
        isbn10: None,
        title: title.to_string(),
        authors: authors.iter().map(|a| a.to_string()).collect(),
        year: None,
        pages: None,
        publisher: None,
        description: None,
        cover_url: None,
        series: None,
        series_index: None,
        first_publish_year: None,
        genres: Vec::new(),
        relevance: None,
    }
}

fn with_isbn(title: &str, isbn13: &str) -> ProviderEdition {
    ProviderEdition {
        isbn13: Some(isbn13.to_string()),
        ..candidate(title, &[])
    }
}

// ── normalization ────────────────────────────────────────────────

#[test]
fn normalize_title_text_folds_punctuation_case_and_runs_of_space() {
    assert_eq!(
        normalize_title_text("  The   Hobbit: Or, There & Back Again!  "),
        "the hobbit or there back again"
    );
}

#[test]
fn normalize_title_text_strips_diacritics_so_transliterations_agree() {
    assert_eq!(normalize_title_text("Sōseki"), "soseki");
    assert_eq!(normalize_title_text("Les Misérables"), "les miserables");
    // The Cyrillic и-breve decomposes and loses its breve, the same rule.
    assert_eq!(normalize_title_text("Война и мир"), "воина и мир");
}

#[test]
fn normalize_title_text_keeps_non_latin_scripts_intact() {
    // Restricting to `[a-z0-9]` would empty these, making two providers
    // reporting the identical title look like different books.
    assert_eq!(normalize_title_text("こころ"), "こころ");
    assert_eq!(normalize_title_text("夏目漱石"), "夏目漱石");
}

#[test]
fn normalize_title_text_keeps_indic_vowel_signs_rather_than_splitting_the_word() {
    // The deliberate divergence from the source implementation, whose
    // character class drops spacing marks and shatters the word: "ह नद".
    let folded = normalize_title_text("हिन्दी");
    assert!(
        !folded.contains(' '),
        "spacing marks must not split the word: {folded:?}"
    );
    assert!(folded.starts_with('ह'), "got {folded:?}");
}

#[test]
fn normalize_title_text_applies_compatibility_decomposition() {
    assert_eq!(normalize_title_text("ﬁnale"), "finale");
    assert_eq!(normalize_title_text("ＡＢＣ"), "abc");
}

#[test]
fn tokenize_drops_single_character_tokens() {
    assert_eq!(
        tokenize(&normalize_title_text("a tale of 2 cities")),
        vec!["tale", "of", "cities"]
    );
}

// ── the title tiers ──────────────────────────────────────────────

#[test]
fn score_title_match_scores_an_exact_match_highest() {
    assert_eq!(score_title_match("Dune", "dune"), 10.0);
}

#[test]
fn score_title_match_scores_a_whole_word_prefix_in_either_direction() {
    assert_eq!(score_title_match("Dune", "Dune Messiah"), 8.0);
    assert_eq!(score_title_match("Dune Messiah", "Dune"), 8.0);
}

#[test]
fn score_title_match_does_not_treat_a_partial_word_as_a_prefix() {
    // "dune" is not a prefix of "dunes" on a word boundary; it falls through
    // to the fuzzy tier rather than scoring 8.
    assert!(score_title_match("Dune", "Dunes of Arrakis") < 8.0);
}

#[test]
fn score_title_match_scores_whole_word_containment_below_a_prefix() {
    assert_eq!(score_title_match("Kings", "The Way of Kings"), 7.0);
}

#[test]
fn score_title_match_requires_more_than_half_the_query_words_to_overlap() {
    // "The Way of Kings" vs "The Way We Were" share exactly one significant
    // word out of two — a ratio of 0.5, which the strict `>` must reject.
    let score = score_title_match("The Way of Kings", "The Way We Were");
    assert!(
        score < MIN_RELEVANCE_SCORE,
        "a half-overlap must not survive, got {score}"
    );
}

#[test]
fn score_title_match_scores_a_clear_token_overlap() {
    // Three of three significant query words present → ratio 1.0 × 6.
    let score = score_title_match("Kings Way Stormlight", "Stormlight Kings Way Archive");
    assert!((score - 6.0).abs() < 1e-9, "got {score}");
}

#[test]
fn score_title_match_tolerates_a_typo_through_the_fuzzy_tier() {
    // "foundatun" vs "foundation": distance 2 over a max length of 10 →
    // similarity 0.8 → 0.8 × 4 = 3.2, which clears the relevance floor.
    let score = score_title_match("Foundatun", "Foundation");
    assert!((score - 3.2).abs() < 1e-9, "got {score}");
    assert!(score >= MIN_RELEVANCE_SCORE);
}

#[test]
fn score_title_match_rejects_two_unrelated_titles_of_similar_length() {
    assert_eq!(score_title_match("Dune", "Emma"), 0.0);
}

#[test]
fn score_title_match_scores_nothing_when_either_side_is_empty() {
    assert_eq!(score_title_match("", "Dune"), 0.0);
    assert_eq!(score_title_match("Dune", "   "), 0.0);
}

#[test]
fn score_title_match_matches_a_title_that_is_all_stopwords() {
    // `significant` falls back to the raw tokens, so a book titled "It" is
    // still matchable rather than scoring zero against everything.
    assert_eq!(score_title_match("It", "It"), 10.0);
}

// ── candidate scoring ────────────────────────────────────────────

#[test]
fn score_candidate_returns_the_isbn_score_on_an_exact_identifier_match() {
    let query = SearchQuery::new(Some("something else entirely"), None, Some("9780134685991"));
    let edition = with_isbn("Nothing Like The Query", "9780134685991");
    assert_eq!(score_candidate(&edition, &query), ISBN_MATCH_SCORE);
}

#[test]
fn score_candidate_matches_an_isbn_carried_only_as_an_isbn_10() {
    let query = SearchQuery::new(None, None, Some("9780134685991"));
    let edition = ProviderEdition {
        isbn13: None,
        isbn10: Some("0134685997".into()),
        ..candidate("Effective Java", &[])
    };
    assert_eq!(score_candidate(&edition, &query), ISBN_MATCH_SCORE);
}

#[test]
fn score_candidate_adds_the_author_only_when_the_title_already_scored() {
    let query = SearchQuery::new(Some("Dune"), Some("Frank Herbert"), None);
    let matching = candidate("Dune", &["Frank Herbert"]);
    assert_eq!(
        score_candidate(&matching, &query),
        10.0 + AUTHOR_MATCH_SCORE
    );

    // Same author, unrelated title: a different book by the same person, which
    // must not be offered.
    let other = candidate("The Santaroga Barrier", &["Frank Herbert"]);
    assert_eq!(score_candidate(&other, &query), 0.0);
}

#[test]
fn score_candidate_scores_a_partial_author_name_lower_than_a_containment() {
    let query = SearchQuery::new(Some("Dune"), Some("Frank Herbert"), None);
    let shared_token = candidate("Dune", &["Herbert Wells"]);
    assert_eq!(
        score_candidate(&shared_token, &query),
        10.0 + AUTHOR_TOKEN_MATCH_SCORE
    );
}

#[test]
fn score_candidate_ignores_the_author_when_the_query_names_none() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    assert_eq!(
        score_candidate(&candidate("Dune", &["Anyone"]), &query),
        10.0
    );
}

// ── filter_and_rank ──────────────────────────────────────────────

#[test]
fn filter_and_rank_drops_a_study_guide_rather_than_ranking_it_lower() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    let found = filter_and_rank(
        vec![
            candidate("A Study Guide for Frank Herbert's Dune", &[]),
            candidate("Summary of Dune", &[]),
            candidate("Dune", &[]),
        ],
        &query,
        10,
    );
    let titles: Vec<&str> = found.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["Dune"]);
}

#[test]
fn filter_and_rank_drops_coincidental_matches_below_the_floor() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    let found = filter_and_rank(vec![candidate("Emma", &[])], &query, 10);
    assert!(found.is_empty());
}

#[test]
fn filter_and_rank_orders_by_score_descending() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    let found = filter_and_rank(
        vec![
            candidate("Dune Messiah", &[]),
            candidate("Dune", &[]),
            candidate("The Road to Dune", &[]),
        ],
        &query,
        10,
    );
    assert_eq!(found[0].title, "Dune", "the exact match must lead");
    assert!(found.len() >= 2);
    assert!(found[0].relevance >= found[1].relevance);
}

#[test]
fn filter_and_rank_stamps_each_survivor_with_its_score() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    let found = filter_and_rank(vec![candidate("Dune", &[])], &query, 10);
    // Hundredths of a point: a perfect title match is 10.0.
    assert_eq!(found[0].relevance, Some(1000));
}

#[test]
fn filter_and_rank_keeps_provider_order_between_candidates_that_tie() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    let mut first = candidate("Dune", &[]);
    first.provider_ref = "first".into();
    let mut second = candidate("Dune", &[]);
    second.provider_ref = "second".into();
    let found = filter_and_rank(vec![first, second], &query, 10);
    let refs: Vec<&str> = found.iter().map(|e| e.provider_ref.as_str()).collect();
    assert_eq!(refs, vec!["first", "second"], "the sort must be stable");
}

#[test]
fn filter_and_rank_caps_at_the_limit_after_sorting() {
    let query = SearchQuery::new(Some("Dune"), None, None);
    let found = filter_and_rank(
        vec![
            candidate("Dune Messiah", &[]),
            candidate("Dune", &[]),
            candidate("Dune Reprint", &[]),
        ],
        &query,
        1,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "Dune", "the cap applies after the sort");
}

#[test]
fn filter_and_rank_returns_candidates_unscored_when_the_query_has_no_title() {
    // The author bonus only applies on top of a title signal, so scoring an
    // author-only query would give every candidate zero and discard the lot.
    // Reachable from the picker with no hand-crafted request: a book whose
    // Title field is empty seeds exactly this.
    let query = SearchQuery::new(None, Some("Frank Herbert"), None);
    let found = filter_and_rank(
        vec![
            candidate("Dune", &["Frank Herbert"]),
            candidate("Emma", &[]),
        ],
        &query,
        10,
    );
    assert_eq!(
        found.len(),
        2,
        "nothing to rank against is not the same as no results"
    );
    assert!(found.iter().all(|e| e.relevance.is_none()));
}

#[test]
fn filter_and_rank_keeps_an_exact_isbn_match_the_skip_list_would_have_dropped() {
    // The skip-list is a heuristic about titles; an exact ISBN is a fact about
    // the edition. A reader editing a book that genuinely *is* a study guide
    // must still be able to find it — the query is seeded from the book.
    let query = SearchQuery::new(Some("A Study Guide for Dune"), None, Some("9780134685991"));
    let found = filter_and_rank(
        vec![with_isbn("A Study Guide for Dune", "9780134685991")],
        &query,
        10,
    );
    assert_eq!(
        found.len(),
        1,
        "an exact identifier outranks a title heuristic"
    );
    assert_eq!(found[0].relevance, Some(10000));
}

#[test]
fn score_author_shares_a_name_word_without_stripping_stopwords_from_it() {
    // Name tokens are not title tokens: a stopword inside a name is part of
    // the name, and two-letter fragments collide too often to count.
    let query = SearchQuery::new(Some("Dune"), Some("Ursula Le Guin"), None);
    let shared = candidate("Dune", &["Ursula Todd"]);
    assert_eq!(
        score_candidate(&shared, &query),
        10.0 + AUTHOR_TOKEN_MATCH_SCORE,
        "a shared name word counts"
    );
    // "Le" is two characters, so it is not a word worth matching on.
    let only_short = candidate("Dune", &["Le Corbusier"]);
    assert_eq!(score_candidate(&only_short, &query), 10.0);
}

#[test]
fn contains_word_keeps_scanning_past_a_non_boundary_hit() {
    // The boundary-aligned occurrence overlaps an earlier one, so a scan that
    // advanced by the needle's whole length would never examine it.
    assert!(contains_word("won on on", "on on"));
    assert!(!contains_word("wondrous", "on"));
}
