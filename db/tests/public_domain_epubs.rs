//! Regression test for the public-domain Playwright EPUB fixtures.
//!
//! These EPUBs come from third-party sources (Project Gutenberg / Standard
//! Ebooks) so we don't control their OPFs. Pinning the metadata the parser
//! extracts from each one means a parser change that subtly drops a field
//! fails here before the Playwright suite gets to it.

use std::collections::HashMap;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test_data")
        .join("epubs")
        .join("public_domain")
}

/// One row per file in `test_data/epubs/public_domain/`. Mirrors the
/// `public_domain/*` entries in `ui_tests/playwright/tests/fixtures/epubs.ts`;
/// keep both in sync when adding or renaming a fixture. PG releases don't
/// set `publisher` or `series`, so those columns aren't asserted here.
struct Expected {
    filename: &'static str,
    title: &'static str,
    authors: &'static [&'static str],
    language: &'static str,
    published: Option<&'static str>,
    has_cover: bool,
}

const EXPECTED: &[Expected] = &[
    Expected {
        filename: "dracula.epub",
        title: "Dracula",
        authors: &["Bram Stoker"],
        language: "en",
        published: Some("1995-10-01"),
        has_cover: true,
    },
    Expected {
        filename: "frankenstein.epub",
        title: "Frankenstein; or, the modern prometheus",
        authors: &["Mary Wollstonecraft Shelley"],
        language: "en",
        published: Some("1993-10-01"),
        has_cover: true,
    },
    Expected {
        filename: "great_gatsby.epub",
        title: "The Great Gatsby",
        authors: &["F. Scott Fitzgerald"],
        language: "en",
        published: Some("2021-01-17"),
        has_cover: true,
    },
    Expected {
        filename: "romeo_and_juliet.epub",
        title: "Romeo and Juliet",
        authors: &["William Shakespeare"],
        language: "en",
        published: Some("1998-11-01"),
        has_cover: true,
    },
    Expected {
        filename: "count_of_monte_cristo.epub",
        title: "The Count of Monte Cristo",
        authors: &["Alexandre Dumas", "Auguste Maquet"],
        language: "en",
        published: Some("1998-01-01"),
        has_cover: true,
    },
    Expected {
        filename: "pride_and_prejudice.epub",
        title: "Pride and Prejudice",
        authors: &["Jane Austen"],
        language: "en",
        published: Some("1998-06-01"),
        has_cover: true,
    },
    Expected {
        filename: "middlemarch.epub",
        title: "Middlemarch",
        authors: &["George Eliot"],
        language: "en",
        published: Some("1994-07-01"),
        has_cover: true,
    },
    Expected {
        filename: "picture_of_dorian_gray.epub",
        title: "The Picture of Dorian Gray",
        authors: &["Oscar Wilde"],
        language: "en",
        published: Some("1994-10-01"),
        has_cover: true,
    },
    Expected {
        filename: "civics_and_health.epub",
        title: "Civics and Health",
        authors: &["William H. Allen"],
        language: "en",
        published: Some("2007-05-08"),
        has_cover: true,
    },
    Expected {
        filename: "tribes_and_castes_of_india.epub",
        title: "The Tribes and Castes of the Central Provinces of India, Volume 2",
        authors: &["R. V. Russell"],
        language: "en",
        published: Some("2007-07-06"),
        has_cover: true,
    },
    Expected {
        filename: "woordenlijst_nederlandsche_taal.epub",
        title: "Woordenlijst voor de spelling der Nederlandsche Taal / Met aanwijzing van de geslachten der naamwoorden en de vervoeging der werkwoorden",
        authors: &["M. de Vries", "L. A. te Winkel"],
        language: "nl",
        published: Some("2007-09-22"),
        has_cover: true,
    },
    Expected {
        filename: "sir_richard_calmady.epub",
        title: "The History of Sir Richard Calmady: A Romance",
        authors: &["Lucas Malet"],
        language: "en",
        published: Some("2007-12-09"),
        has_cover: true,
    },
    Expected {
        filename: "mariucha.epub",
        title: "Mariucha",
        authors: &["Benito Pérez Galdós"],
        // OPF claims `en` even though the work is Spanish; we record what the file says.
        language: "en",
        published: Some("2008-03-25"),
        has_cover: true,
    },
    Expected {
        filename: "life_of_charles_dickens.epub",
        title: "The Life of Charles Dickens, Vol. I-III, Complete",
        authors: &["John Forster"],
        language: "en",
        published: Some("2008-06-20"),
        has_cover: true,
    },
    Expected {
        filename: "charles_frohman.epub",
        title: "Charles Frohman: Manager and Man",
        authors: &["Isaac Frederick Marcosson", "Daniel Frohman"],
        language: "en",
        published: Some("2008-07-29"),
        has_cover: true,
    },
    Expected {
        filename: "room_with_a_view.epub",
        title: "A Room with a View",
        authors: &["E. M. Forster"],
        language: "en",
        published: Some("2001-05-01"),
        has_cover: true,
    },
    Expected {
        filename: "works_of_george_gillespie.epub",
        title: "The Works of Mr. George Gillespie (Vol. 1 of 2)",
        authors: &["George Gillespie"],
        language: "en",
        published: Some("2008-10-08"),
        has_cover: true,
    },
    Expected {
        filename: "moby_dick.epub",
        title: "Moby Dick; Or, The Whale",
        authors: &["Herman Melville"],
        language: "en",
        published: Some("2001-07-01"),
        has_cover: true,
    },
    Expected {
        filename: "little_women.epub",
        title: "Little Women; Or, Meg, Jo, Beth, and Amy",
        authors: &["Louisa May Alcott"],
        language: "en",
        published: Some("2011-08-16"),
        has_cover: true,
    },
    Expected {
        filename: "gogol_dramatische_werke.epub",
        title: "Sämmtliche Werke 5: Dramatische Werke",
        authors: &["Nikolai Vasilevich Gogol"],
        language: "de",
        published: Some("2017-09-05"),
        has_cover: true,
    },
    Expected {
        filename: "wuthering_heights.epub",
        title: "Wuthering Heights",
        authors: &["Emily Brontë"],
        language: "en",
        published: Some("1996-12-01"),
        has_cover: true,
    },
];

#[test]
fn public_domain_epubs_parse_with_expected_metadata() {
    let dir = fixtures_dir();
    let result = omnibus_db::ebook::scan_ebook_library(Some(dir.to_str().unwrap()));
    assert!(result.error.is_none(), "scan errored: {:?}", result.error);
    assert_eq!(
        result.books.len(),
        EXPECTED.len(),
        "public-domain fixture count drifted: scanner found {}, EXPECTED has {}",
        result.books.len(),
        EXPECTED.len(),
    );

    let by_name: HashMap<&str, _> = result
        .books
        .iter()
        .map(|b| (b.metadata.filename.as_str(), b))
        .collect();

    for exp in EXPECTED {
        let book = by_name
            .get(exp.filename)
            .unwrap_or_else(|| panic!("fixture {} missing from scan", exp.filename));
        let m = &book.metadata;
        assert!(
            m.error.is_none(),
            "{} parse error: {:?}",
            exp.filename,
            m.error,
        );
        assert_eq!(
            m.title.as_deref(),
            Some(exp.title),
            "{} title",
            exp.filename
        );
        let actual_authors: Vec<&str> = m.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(actual_authors, exp.authors, "{} authors", exp.filename);
        assert_eq!(
            m.language.as_deref(),
            Some(exp.language),
            "{} language",
            exp.filename,
        );
        assert_eq!(
            m.published.as_deref(),
            exp.published,
            "{} published",
            exp.filename,
        );
        assert_eq!(
            book.cover.is_some(),
            exp.has_cover,
            "{} cover presence",
            exp.filename,
        );
    }
}
