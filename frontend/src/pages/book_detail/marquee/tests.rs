//! The running order is a cross-client contract. iOS pins the same six
//! sections in the same order (`detailStopsRunHomeToMoreInSixSections` in
//! `omnibus-ios/omnibusTests/`), so a reorder on this side has to fail this
//! build too — a guard on one client notices drift, it doesn't stop it.

use super::MARQUEE_SECTIONS;

#[test]
fn marquee_sections_run_home_to_more_in_six_sections() {
    let names: Vec<&str> = MARQUEE_SECTIONS.iter().map(|(_, name)| *name).collect();
    assert_eq!(
        names,
        [
            "Home",
            "Stats",
            "Highlights",
            "Journals",
            "The files",
            "More"
        ]
    );
}

#[test]
fn marquee_section_numbers_match_their_position() {
    // The `NN` is written out rather than derived from the index, and it is
    // rendered in two places (the dot rail and the section label). A reorder
    // that moved a row without renumbering it would leave both reading a
    // position the section no longer holds.
    for (i, (no, name)) in MARQUEE_SECTIONS.iter().enumerate() {
        assert_eq!(
            *no,
            format!("{:02}", i + 1),
            "{name} carries the wrong number for its position"
        );
    }
}
