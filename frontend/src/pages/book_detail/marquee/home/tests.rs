use omnibus_shared::EbookMetadata;

use super::*;

fn facts(has_ebook: bool, has_audio: bool, has_comic: bool) -> MarqueeViewFacts {
    MarqueeViewFacts {
        title: "Book".into(),
        primary_author: "Author".into(),
        author_id: None,
        authors_line: "Author".into(),
        series: None,
        has_ebook,
        has_audio,
        has_comic,
    }
}

fn book() -> EbookMetadata {
    EbookMetadata {
        unique_identifier: Some("book-uuid".into()),
        ..Default::default()
    }
}

fn no_progress() -> MarqueeProgress {
    MarqueeProgress {
        reading: None,
        listening: None,
    }
}

/// SSR-render the CTA row for a book with the given format availability.
fn render_cta_row(has_ebook: bool, has_audio: bool) -> String {
    dioxus::ssr::render_element(rsx! {
        MarqueeCtaRow {
            b: book(),
            view: facts(has_ebook, has_audio, false),
            progress: no_progress(),
        }
    })
}

#[test]
fn immersive_cta_renders_when_book_has_both_ebook_and_audio() {
    let html = render_cta_row(true, true);
    assert!(html.contains("data-testid=\"immersive-read\""));
    // The marquee row uses the design's terser label.
    assert!(html.contains(">Immersive<"), "{html}");
}

#[test]
fn immersive_cta_absent_when_book_has_ebook_only() {
    let html = render_cta_row(true, false);
    assert!(!html.contains("data-testid=\"immersive-read\""));
}

#[test]
fn immersive_cta_absent_when_book_has_audio_only() {
    let html = render_cta_row(false, true);
    assert!(!html.contains("data-testid=\"immersive-read\""));
}

#[test]
fn fileless_book_shows_no_files_disclaimer_and_hides_reading_ctas() {
    let html = render_cta_row(false, false);
    assert!(html.contains("data-testid=\"no-files-disclaimer\""));
    assert!(!html.contains("data-testid=\"start-reading\""));
    assert!(!html.contains("data-testid=\"start-listening\""));
}

#[test]
fn book_with_files_shows_no_disclaimer() {
    let html = render_cta_row(true, false);
    assert!(!html.contains("data-testid=\"no-files-disclaimer\""));
}

// The comic CTA (and the single-file picker) render `dioxus_router::Link`,
// which panics without a parent router, so these two variants mount
// behind one-route test routers — the highlights-card pattern.
#[derive(Clone, Debug, PartialEq, dioxus_router::Routable)]
enum ComicOnlyRoute {
    #[route("/")]
    ComicOnlyHost {},
}

#[component]
fn ComicOnlyHost() -> Element {
    rsx! {
        MarqueeCtaRow {
            b: book(),
            view: facts(false, false, true),
            progress: no_progress(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, dioxus_router::Routable)]
enum EpubAndComicRoute {
    #[route("/")]
    EpubAndComicHost {},
}

#[component]
fn EpubAndComicHost() -> Element {
    rsx! {
        MarqueeCtaRow {
            b: book(),
            view: facts(true, false, true),
            progress: no_progress(),
        }
    }
}

#[test]
fn comic_only_book_shows_the_pager_cta_instead_of_the_disclaimer() {
    let html = crate::test_support::render_in_vdom(|| {
        rsx! {
            dioxus_router::Router::<ComicOnlyRoute> {}
        }
    });
    assert!(
        html.contains("data-testid=\"start-reading-comic\""),
        "{html}"
    );
    assert!(html.contains("/comic/book-uuid"), "{html}");
    assert!(
        !html.contains("data-testid=\"no-files-disclaimer\""),
        "{html}"
    );
}

#[test]
fn epub_primary_wins_over_the_comic_cta_when_both_formats_exist() {
    let html = crate::test_support::render_in_vdom(|| {
        rsx! {
            dioxus_router::Router::<EpubAndComicRoute> {}
        }
    });
    assert!(html.contains("data-testid=\"start-reading\""), "{html}");
    assert!(
        !html.contains("data-testid=\"start-reading-comic\""),
        "{html}"
    );
}

// The single-file picker renders a `Link`, so the resume-verb variant mounts
// behind a one-route test router like the comic hosts above.
#[derive(Clone, Debug, PartialEq, dioxus_router::Routable)]
enum ResumeRoute {
    #[route("/")]
    ResumeHost {},
}

#[component]
fn ResumeHost() -> Element {
    use omnibus_shared::progress::{ProgressFormat, ProgressRecord};
    let progress = MarqueeProgress {
        reading: Some(ProgressRecord {
            book_uuid: "book-uuid".into(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(40),
            kobo_location: None,
            book_file_id: None,
            updated_at: 0,
            client_updated_at: 0,
            total_duration_seconds: None,
            resolved: None,
        }),
        listening: None,
    };
    rsx! {
        MarqueeCtaRow {
            b: book(),
            view: facts(true, false, false),
            progress,
        }
    }
}

#[test]
fn resume_verbs_take_over_once_a_position_exists() {
    let html = crate::test_support::render_in_vdom(|| {
        rsx! {
            dioxus_router::Router::<ResumeRoute> {}
        }
    });
    assert!(html.contains("Resume reading"), "{html}");
}

#[test]
fn short_chapter_title_keeps_a_terse_title_and_caps_a_verbose_one() {
    assert_eq!(short_chapter_title("Cinder"), "Cinder");
    assert_eq!(
        short_chapter_title("CHAPTER XIII DR. SEWARD'S DIARY\u{2014}continued"),
        "CHAPTER XIII DR. SEWARD'S\u{2026}"
    );
    // Breaks on a word boundary and trims a trailing dash/comma.
    assert_eq!(
        short_chapter_title("The Vestibule of the Sixteenth Hall, north"),
        "The Vestibule of the Sixteenth\u{2026}"
    );
}

#[test]
fn chapter_now_prefers_the_cfi_spine_over_the_rounded_percent() {
    use omnibus_shared::progress::{ProgressFormat, ProgressRecord};
    // Two adjacent chapters whose starts round to the same integer percent:
    // the reader is in the first (spine 4), but a rounded 9% pulls the label
    // one ahead to the second (spine 6).
    let chapters = [
        AlignmentEbookChapter {
            title: "Front".into(),
            percent: 0.0,
            spine_index: 0,
        },
        AlignmentEbookChapter {
            title: "Chapter One".into(),
            percent: 8.4,
            spine_index: 4,
        },
        AlignmentEbookChapter {
            title: "Chapter Two".into(),
            percent: 8.9,
            spine_index: 6,
        },
    ];
    let rec = |cfi: Option<&str>, pct: Option<i64>| ProgressRecord {
        book_uuid: "b".into(),
        format: ProgressFormat::Epub,
        epub_cfi: cfi.map(Into::into),
        audio_position_seconds: None,
        progress_percent: pct,
        kobo_location: None,
        book_file_id: None,
        updated_at: 0,
        client_updated_at: 0,
        total_duration_seconds: None,
        resolved: None,
    };

    // CFI in spine item 4 (package step 10 → ordinal 5 → 0-based 4) → Chapter One.
    let r = rec(Some("epubcfi(/6/10!/4/2:0)"), Some(9));
    assert_eq!(
        chapter_now(&chapters, Some(&r)),
        Some((2, "Chapter One".into()))
    );

    // No CFI: the rounded 9% falls back and lands one chapter ahead — the old
    // behaviour the CFI path corrects.
    let r = rec(None, Some(9));
    assert_eq!(
        chapter_now(&chapters, Some(&r)),
        Some((3, "Chapter Two".into()))
    );

    // No position at all → no chapter named.
    assert_eq!(chapter_now(&chapters, Some(&rec(None, Some(0)))), None);
    assert_eq!(chapter_now(&chapters, None), None);
}

/// A series book with the given index, series name, and published date.
fn series_book(index: Option<&str>, published: Option<&str>) -> EbookMetadata {
    EbookMetadata {
        series: Some("Prince of Sin".into()),
        series_index: index.map(str::to_string),
        published: published.map(str::to_string),
        ..book()
    }
}

#[test]
fn home_kicker_never_reads_the_series_index_as_a_fraction_of_the_library() {
    // The reported "PRINCE OF SIN · BOOK 2 OF 1": the index is a position in
    // the series and the count is what the library holds, so a book the
    // library holds one of still sits at position 2.
    let kicker = home_kicker(
        &series_book(Some("2"), Some("2024-03-05")),
        &facts(true, false, false),
        Some(1),
        false,
        None,
    );
    assert_eq!(
        kicker.text,
        "Prince of Sin \u{b7} Book 2 \u{b7} 1 in your library \u{b7} 2024"
    );
    assert_eq!(kicker.series_label.as_deref(), Some("Prince of Sin"));
}

#[test]
fn home_kicker_matches_the_shelf_stops_wording_for_the_library_count() {
    // The kicker and the Shelf stop's header must present the series the
    // same way — see `MarqueeSeriesShelf`'s "· N in your library".
    let kicker = home_kicker(
        &series_book(Some("1"), None),
        &facts(true, false, false),
        Some(3),
        false,
        None,
    );
    assert!(kicker.tail.contains("3 in your library"), "{}", kicker.tail);
}

#[test]
fn home_kicker_names_the_position_alone_before_the_series_fetch_resolves() {
    let kicker = home_kicker(
        &series_book(Some("2"), None),
        &facts(true, false, false),
        None,
        false,
        None,
    );
    assert_eq!(kicker.text, "Prince of Sin \u{b7} Book 2");
}

#[test]
fn home_kicker_drops_an_implausible_published_year() {
    // Calibre's "no publish date" placeholder must not surface as "· 0101".
    let kicker = home_kicker(
        &series_book(Some("2"), Some("0101-01-01T00:00:00+00:00")),
        &facts(true, false, false),
        Some(1),
        false,
        None,
    );
    assert!(!kicker.text.contains("0101"), "{}", kicker.text);
}

#[test]
fn home_kicker_leads_a_standalone_with_its_category_and_year() {
    let b = EbookMetadata {
        genres: vec!["Horror".into()],
        published: Some("2015-01-01T05:00:00+00:00".into()),
        ..book()
    };
    let kicker = home_kicker(&b, &facts(true, false, false), None, false, None);
    assert_eq!(kicker.text, "Horror \u{b7} standalone \u{b7} 2015");
    assert!(kicker.series_label.is_none());
}

#[test]
fn home_kicker_treats_a_cleared_series_as_a_standalone() {
    // An override that empties the series name left `series: Some("")`
    // taking the series branch, which rendered a nameless "· Book 3"
    // crumb linking at a series the book is no longer in (#2349).
    let b = EbookMetadata {
        series: Some(String::new()),
        series_index: Some("3".into()),
        genres: vec!["Horror".into()],
        ..book()
    };
    let kicker = home_kicker(&b, &facts(true, false, false), Some(4), false, None);
    assert_eq!(kicker.text, "Horror \u{b7} standalone");
    assert!(kicker.series_label.is_none());
    assert!(!kicker.text.contains("Book 3"), "{}", kicker.text);
}

#[test]
fn description_overflows_only_past_the_clamps_worth_of_words() {
    let short = "A dozen words is nowhere near five rendered lines of blurb text.";
    assert!(!description_overflows(short));
    let long = "word ".repeat(DESC_CLAMP_WORDS + 1);
    assert!(description_overflows(&long));
    // Markup is not text — a heavily tagged short blurb must not trip it.
    let tagged = "<em>one</em> <em>two</em> <em>three</em>".repeat(3);
    assert!(!description_overflows(&tagged));
}
