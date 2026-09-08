//! The pure detector building blocks: the normalized-key fold, Jaccard
//! scoring and first-token blocking, `pick_canonical`, the order-stable
//! merge payload, `split_candidate`'s tiers, and pattern compilation with
//! its error variant.

use super::super::*;
use super::merge_payload;

// Pure unit tests: tier0_key
#[test]
fn tier0_key_collapses_last_first_swap_the_same_via_sort_or_name() {
    let via_name = tier0_key("Sanderson, Brandon", None);
    let via_plain = tier0_key("Brandon Sanderson", None);
    let via_sort = tier0_key("junk display name", Some("Sanderson, Brandon"));
    assert_eq!(via_name, Some("brandon sanderson".to_string()));
    assert_eq!(via_name, via_plain);
    assert_eq!(via_name, via_sort, "sort must win over name when present");
}

#[test]
fn tier0_key_returns_none_when_nothing_survives() {
    assert_eq!(tier0_key("---", None), None);
}

// Pure unit tests: Jaccard + first-token blocking
#[test]
fn jaccard_scores_full_overlap_partial_overlap_and_disjoint_sets() {
    let a = token_set("alpha bravo charlie");
    let b = token_set("alpha bravo charlie");
    assert_eq!(jaccard(&a, &b), 1.0);

    let c = token_set("alpha bravo charlie delta echo foxtrot golf");
    let d = token_set("alpha bravo charlie delta echo foxtrot golf hotel");
    assert_eq!(jaccard(&c, &d), 7.0 / 8.0);

    let e = token_set("alpha bravo");
    let f = token_set("charlie delta");
    assert_eq!(jaccard(&e, &f), 0.0);
}

#[test]
fn fuzzy_merge_suggestions_blocks_pairs_with_different_first_token() {
    // Both keys share 12 of 14 total tokens (>= 0.85 Jaccard if compared),
    // but differ in their *first* token — first-token blocking must keep
    // them in separate buckets and never compare them.
    let key_a = "quebec charlie delta echo foxtrot golf hotel india juliet kilo lima mike november";
    let key_b = "romeo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november";
    assert!(jaccard(&token_set(key_a), &token_set(key_b)) >= FUZZY_JACCARD_THRESHOLD);

    let row_a = EntityRow {
        id: 1,
        name: "A".into(),
        sort: None,
        book_ids: HashSet::new(),
    };
    let row_b = EntityRow {
        id: 2,
        name: "B".into(),
        sort: None,
        book_ids: HashSet::new(),
    };
    let singles = [(key_a, &row_a), (key_b, &row_b)];
    assert!(fuzzy_merge_suggestions(CleanupKind::Author, &singles).is_empty());
}

// Pure unit tests: pick_canonical
#[test]
fn pick_canonical_prefers_more_books_then_lower_id() {
    let few_books = EntityRow {
        id: 5,
        name: "Few".into(),
        sort: None,
        book_ids: HashSet::from([101]),
    };
    let many_books = EntityRow {
        id: 9,
        name: "Many".into(),
        sort: None,
        book_ids: HashSet::from([201, 202, 203, 204]),
    };
    let group = [&few_books, &many_books];
    assert_eq!(pick_canonical(&group).unwrap().id, many_books.id);

    let tie_high_id = EntityRow {
        id: 9,
        name: "High".into(),
        sort: None,
        book_ids: HashSet::from([301]),
    };
    let tie_low_id = EntityRow {
        id: 2,
        name: "Low".into(),
        sort: None,
        book_ids: HashSet::from([302]),
    };
    let tied = [&tie_high_id, &tie_low_id];
    assert_eq!(pick_canonical(&tied).unwrap().id, tie_low_id.id);
}

#[test]
fn pick_canonical_returns_none_for_an_empty_group() {
    let empty: [&EntityRow; 0] = [];
    assert!(pick_canonical(&empty).is_none());
}

// Pure unit tests: merge_suggestion payload ordering
/// `dedup_suggestions`'s `UNIQUE (kind, action, payload_json)` (migration
/// `0069`) is a plain string comparison over the serialized payload, so
/// `merge_suggestion` must emit `source_ids`/`source_names` in a stable
/// order regardless of the order `group` arrives in. `group` traces back to
/// `fetch_entity_rows`'s `HashMap<i64, EntityRow>::into_values()`, whose
/// iteration order is not stable across runs — without the id-sort in
/// `merge_suggestion`, the same logical merge would serialize differently
/// on a re-run and defeat `INSERT OR IGNORE`, piling up duplicate rows.
#[test]
fn merge_suggestion_serializes_identical_payload_json_regardless_of_group_order() {
    let canonical = EntityRow {
        id: 1,
        name: "Canonical".into(),
        sort: None,
        book_ids: HashSet::from([100]),
    };
    let dup_a = EntityRow {
        id: 2,
        name: "Dup A".into(),
        sort: None,
        book_ids: HashSet::from([101]),
    };
    let dup_b = EntityRow {
        id: 3,
        name: "Dup B".into(),
        sort: None,
        book_ids: HashSet::from([102]),
    };

    // Same logical group, two different arrival orders — standing in for
    // the two different `HashMap` iteration orders the real detector could
    // hand `merge_suggestion` across two separate runs.
    let forward = [&canonical, &dup_a, &dup_b];
    let reversed = [&canonical, &dup_b, &dup_a];

    let s1 = merge_suggestion(CleanupKind::Author, Tier::Zero, 1.0, &forward, &canonical);
    let s2 = merge_suggestion(CleanupKind::Author, Tier::Zero, 1.0, &reversed, &canonical);

    let json1 = serde_json::to_string(&s1.payload).unwrap();
    let json2 = serde_json::to_string(&s2.payload).unwrap();
    assert_eq!(
        json1, json2,
        "payload_json must match regardless of group order, or INSERT OR IGNORE's \
         UNIQUE (kind, action, payload_json) constraint silently stops deduplicating"
    );

    let (source_ids, source_names, ..) = merge_payload(&s1);
    assert_eq!(
        source_ids,
        [dup_a.id, dup_b.id],
        "source_ids should be sorted ascending by id"
    );
    assert_eq!(source_names, [dup_a.name.clone(), dup_b.name.clone()]);
}

// Pure unit tests: split_candidate
#[test]
fn split_candidate_detects_semicolon_soup_as_tier0() {
    let (tier, delimiter, atoms) =
        split_candidate("Fantasy Romance; Fantasy New Adult; Fantasy").unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(delimiter, ";");
    assert_eq!(
        atoms,
        vec!["Fantasy Romance", "Fantasy New Adult", "Fantasy"]
    );
}

#[test]
fn split_candidate_detects_long_embedded_comma_name_as_tier1() {
    let name = "Epic High Fantasy, Sword and Sorcery, Heroic Journey Fiction";
    assert!(name.len() >= TAG_SPLIT_LONG_LEN);
    let (tier, delimiter, atoms) = split_candidate(name).unwrap();
    assert_eq!(tier, Tier::One);
    assert_eq!(delimiter, ",");
    assert_eq!(
        atoms,
        vec![
            "Epic High Fantasy",
            "Sword and Sorcery",
            "Heroic Journey Fiction"
        ]
    );
}

#[test]
fn split_candidate_returns_none_for_a_short_plain_tag() {
    assert_eq!(split_candidate("Fantasy"), None);
    // A single short comma-pair isn't "long" enough for the tier-1 heuristic.
    assert_eq!(split_candidate("Sci-Fi, Fantasy"), None);
}

// Pure unit tests: pattern compilation
#[test]
fn compile_all_compiles_every_valid_pattern() {
    assert_eq!(compile_all(&["^a", "b$"]).unwrap().len(), 2);
}

#[test]
#[allow(clippy::invalid_regex)] // deliberately malformed input under test
fn compile_all_returns_err_for_an_invalid_pattern() {
    assert!(compile_all(&["valid", "("]).is_err());
}

#[test]
#[allow(clippy::invalid_regex)] // deliberately malformed input under test
fn cleanup_error_pattern_variant_wraps_an_invalid_regex_error() {
    let regex_err = Regex::new("(").unwrap_err();
    let err: CleanupError = regex_err.into();
    assert!(matches!(err, CleanupError::Pattern(_)));
}
