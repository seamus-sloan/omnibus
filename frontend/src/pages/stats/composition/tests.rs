//! Tests for the library-composition card: the coverage line every panel
//! carries, the format panel's overlap disclosure, and its refusal to draw an
//! axis for a dimension the library has nothing to say about.

use super::*;
use omnibus_shared::MeasuredTotal;

fn slice(label: &str, books: i64) -> CompositionSlice {
    CompositionSlice {
        label: label.into(),
        books,
    }
}

fn dim(slices: &[(&str, i64)], covered: i64) -> CompositionDimension {
    let slices: Vec<CompositionSlice> = slices.iter().map(|(l, b)| slice(l, *b)).collect();
    CompositionDimension {
        coverage: MeasuredTotal {
            total: slices.iter().map(|s| s.books).sum(),
            books: covered,
        },
        slices,
    }
}

/// A library whose every dimension has something to say.
fn full() -> LibraryComposition {
    LibraryComposition {
        books: 1_510,
        ghosted_books: 4,
        // 1,580 placements over 1,510 books, so seventy are held in both.
        formats: CompositionDimension {
            slices: vec![slice("EPUB", 1_400), slice("M4B", 180)],
            coverage: MeasuredTotal {
                total: 1_580,
                books: 1_510,
            },
        },
        languages: dim(&[("eng", 1_180), ("fra", 40)], 1_200),
        publishers: dim(&[("Tor", 90), ("Other", 310)], 400),
        decades: dim(&[("1990s", 200), ("2000s", 620)], 820),
        genres: dim(&[("Fantasy", 40), ("Horror", 22)], 58),
    }
}

#[test]
fn bar_width_scales_to_the_tallest_bar_and_survives_an_empty_dimension() {
    assert_eq!(bar_width(50, 100), 50);
    assert_eq!(bar_width(100, 100), 100);
    // A dimension with nothing in it must not divide by zero on the way to
    // drawing no bars.
    assert_eq!(bar_width(0, 0), 0);
}

#[test]
fn coverage_note_always_states_the_denominator() {
    let genres = dim(&[("Fantasy", 40)], 58);

    assert_eq!(
        coverage_note(&genres, 1_510),
        "across 58 of 1,510 books",
        "a distribution without its coverage is a guess wearing a chart"
    );
}

#[test]
fn overlap_note_appears_only_when_a_book_is_held_in_two_formats() {
    // Two placements over two books: nothing to disclose.
    assert_eq!(overlap_note(&dim(&[("EPUB", 1), ("M4B", 1)], 2)), None);

    let dual = CompositionDimension {
        slices: vec![slice("EPUB", 2), slice("M4B", 1)],
        coverage: MeasuredTotal { total: 3, books: 2 },
    };
    assert_eq!(
        overlap_note(&dual).as_deref(),
        Some("+1 book held in more than one format")
    );
}

#[test]
fn ghosted_note_is_absent_when_every_book_still_has_its_files() {
    assert_eq!(ghosted_note(0), None);
    assert!(ghosted_note(4).unwrap().starts_with("4 books excluded"));
    assert!(ghosted_note(1).unwrap().starts_with("1 book excluded"));
}

#[test]
fn build_panels_gives_the_genre_panel_a_coverage_line_that_names_it_hand_assigned() {
    let panels = build_panels(&full());

    assert_eq!(panels.len(), 5);
    let genres = panels
        .iter()
        .find(|p| p.testid == "stats-composition-genres")
        .unwrap();
    // 58 of 1,510 is the number that stops these slices reading as "your
    // library's genres".
    assert_eq!(
        genres.note.as_deref(),
        Some("hand-assigned \u{2014} across 58 of 1,510 books")
    );
}

#[cfg(feature = "server")]
#[test]
fn composition_card_renders_every_dimension_with_its_coverage() {
    let html =
        crate::test_support::render(rsx! { LibraryCompositionCard { composition: Some(full()) } });

    assert!(html.contains("stats-library-composition"), "{html}");
    for testid in [
        "stats-composition-formats",
        "stats-composition-languages",
        "stats-composition-publishers",
        "stats-composition-decades",
        "stats-composition-genres",
    ] {
        assert!(html.contains(testid), "missing {testid}: {html}");
    }
    // The card title must not read as the period-scoped "How you consumed
    // them" split, which is seconds rather than the shelf's format mix.
    assert!(html.contains("What your library is made of"), "{html}");
    // Every string the E2E spec asserts, verified here against real SSR
    // markup so a renamed label breaks a unit test rather than a browser run.
    for text in [
        "EPUB",
        "1990s",
        "hand-assigned \u{2014} across 58 of 1,510 books",
        // 1,580 placements over 1,510 books: seventy held in both formats.
        "+70 books held in more than one format",
        "stats-composition-bar",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    // Ghosted rows are named rather than left to make the bars not add up.
    assert!(html.contains("stats-composition-ghosted"), "{html}");
    assert!(html.contains("4 books excluded"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn a_dimension_with_no_data_renders_an_empty_state_rather_than_an_empty_chart() {
    let composition = LibraryComposition {
        publishers: CompositionDimension::default(),
        languages: CompositionDimension::default(),
        genres: CompositionDimension::default(),
        decades: CompositionDimension::default(),
        ..full()
    };

    let html = crate::test_support::render(
        rsx! { LibraryCompositionCard { composition: Some(composition) } },
    );

    // The formats panel still has bars, so the card renders — but the four
    // empty dimensions say so in words instead of drawing an axis.
    assert!(html.contains("stats-library-composition"), "{html}");
    assert!(html.contains("No publisher metadata yet."), "{html}");
    assert!(html.contains("No language metadata yet."), "{html}");
    assert!(html.contains("No publication dates yet."), "{html}");
    assert!(html.contains("No genres assigned yet."), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn composition_card_renders_nothing_before_the_fetch_lands_or_for_an_empty_library() {
    let pending =
        crate::test_support::render(rsx! { LibraryCompositionCard { composition: None } });
    assert!(!pending.contains("stats-library-composition"), "{pending}");

    let empty = crate::test_support::render(rsx! {
        LibraryCompositionCard { composition: Some(LibraryComposition::default()) }
    });
    assert!(!empty.contains("stats-library-composition"), "{empty}");
}
