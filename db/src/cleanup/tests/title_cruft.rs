//! `strip_title_cruft`'s regex heuristics: the author, series and
//! acronym prefixes it strips as Tier-0, the trailing parentheticals as
//! Tier-1, the separators it drops, and the real titles it must leave
//! alone.

use super::super::*;

// Pure unit tests: strip_title_cruft
fn title_regexes() -> (Vec<Regex>, Regex) {
    (
        compile_all(TITLE_PREFIX_PATTERNS).unwrap(),
        Regex::new(TITLE_SUFFIX_PATTERN).unwrap(),
    )
}

#[test]
fn strip_title_cruft_strips_last_first_dash_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) = strip_title_cruft(
        "Maas, Sarah J - A Court of Thorns and Roses",
        &prefixes,
        &suffix,
    )
    .unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "A Court of Thorns and Roses");
}

#[test]
fn strip_title_cruft_strips_series_bracket_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) =
        strip_title_cruft("[Mistborn 01] The Final Empire", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_series_hash_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) =
        strip_title_cruft("Mistborn #1 The Final Empire", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_acronym_index_dash_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) = strip_title_cruft("ToG04-Queen of Shadows", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "Queen of Shadows");
}

#[test]
fn strip_title_cruft_strips_trailing_parenthetical_as_tier1() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) =
        strip_title_cruft("The Final Empire (Mistborn, #1)", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::One);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_an_author_and_a_series_prefix_in_one_pass() {
    // Regression: stopping at the first matching prefix left the series
    // segment in the proposal ("Mistborn 01 - The Final Empire").
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) = strip_title_cruft(
        "Sanderson, Brandon - Mistborn 01 - The Final Empire",
        &prefixes,
        &suffix,
    )
    .unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_a_book_marker_series_index_prefix() {
    let (prefixes, suffix) = title_regexes();
    let (_, proposed) =
        strip_title_cruft("Mistborn Book 1 - The Final Empire", &prefixes, &suffix).unwrap();
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_drops_the_separator_a_stripped_prefix_leaves_behind() {
    let (prefixes, suffix) = title_regexes();
    let (_, proposed) =
        strip_title_cruft("Mistborn #1 - The Final Empire", &prefixes, &suffix).unwrap();
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_every_trailing_parenthetical_not_just_the_last() {
    // A scene release stacks them; stripping one left "Berserk v01 (2003)
    // (Digital)" as the proposal.
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) = strip_title_cruft(
        "Berserk v01 (2003) (Digital) (danke-Empire)",
        &prefixes,
        &suffix,
    )
    .unwrap();
    assert_eq!(tier, Tier::One);
    assert_eq!(proposed, "Berserk v01");
}

#[test]
fn strip_title_cruft_returns_none_for_a_title_whose_comma_is_not_an_author_prefix() {
    // Regression: `^[^,]+,\s*[^-]+-\s*` matched "The Life of Charles
    // Dickens, Vol. I-" and proposed "III, Complete".
    let (prefixes, suffix) = title_regexes();
    assert_eq!(
        strip_title_cruft(
            "The Life of Charles Dickens, Vol. I-III, Complete",
            &prefixes,
            &suffix,
        ),
        None
    );
}

#[test]
fn strip_title_cruft_returns_none_for_a_real_title_ending_in_a_number() {
    // The series-index prefix must not fire on an unpadded, unmarked number.
    let (prefixes, suffix) = title_regexes();
    assert_eq!(
        strip_title_cruft("Apollo 13 - The Untold Story", &prefixes, &suffix),
        None
    );
}

#[test]
fn strip_title_cruft_keeps_a_volume_marker_that_carries_no_index_separator() {
    let (prefixes, suffix) = title_regexes();
    assert_eq!(
        strip_title_cruft("Kakuriyo v01 - Waco Ioka", &prefixes, &suffix),
        None
    );
}

#[test]
fn strip_title_cruft_returns_none_for_a_clean_title() {
    let (prefixes, suffix) = title_regexes();
    assert_eq!(strip_title_cruft("Elantris", &prefixes, &suffix), None);
}
