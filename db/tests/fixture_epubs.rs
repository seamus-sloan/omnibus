//! Regression test for the synthetic Playwright EPUB fixtures.
//!
//! Catches drift between `ui_tests/playwright/tools/make_epub.ts` and the OPF
//! parser in `omnibus_db::ebook` without needing to spin up Playwright. If
//! you regenerate the fixtures and the parser disagrees with what the
//! generator wrote, this test fails first — well before the Playwright
//! suite gets to it.

use std::collections::HashMap;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at `db/`; fixtures live at `<repo>/test_data/epubs/generated`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test_data")
        .join("epubs")
        .join("generated")
}

/// One row per file in `test_data/epubs/generated/`. Mirrors the TypeScript
/// `FIXTURE_BOOKS` table in `ui_tests/playwright/tests/fixtures/epubs.ts`;
/// keep both in sync when adding or renaming a fixture.
struct Expected {
    filename: &'static str,
    title: &'static str,
    authors: &'static [&'static str],
    publisher: Option<&'static str>,
    published: Option<&'static str>,
    language: &'static str,
    series: Option<&'static str>,
    series_index: Option<&'static str>,
    has_cover: bool,
}

const EXPECTED: &[Expected] = &[
    Expected {
        filename: "alpha.epub",
        title: "Alpha",
        authors: &["Ada Lovelace"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1843-10-01"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "beta.epub",
        title: "Beta in the Series",
        authors: &["Grace Hopper", "Margaret Hamilton"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1969-07-20"),
        language: "en",
        series: Some("Pioneers"),
        series_index: Some("1"),
        has_cover: true,
    },
    Expected {
        filename: "gamma.epub",
        title: "Gamma sin Cover",
        authors: &["Hedy Lamarr"],
        publisher: Some("Editorial Omnibus"),
        published: Some("1942-08-11"),
        language: "es",
        series: Some("Pioneers"),
        series_index: Some("2"),
        has_cover: false,
    },
    Expected {
        filename: "pioneers-3.epub",
        title: "Pioneers Vol 3: Cipher",
        authors: &["Joan Clarke"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1947-06-12"),
        language: "en",
        series: Some("Pioneers"),
        series_index: Some("3"),
        has_cover: true,
    },
    Expected {
        filename: "pioneers-4.epub",
        title: "Pioneers Vol 4: Loop",
        authors: &["Karen Sparck Jones"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1972-11-02"),
        language: "en",
        series: Some("Pioneers"),
        series_index: Some("4"),
        has_cover: true,
    },
    Expected {
        filename: "pioneers-5.epub",
        title: "Pioneers Vol 5: Signal",
        authors: &["Radia Perlman"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1985-03-15"),
        language: "en",
        series: Some("Pioneers"),
        series_index: Some("5"),
        has_cover: true,
    },
    Expected {
        filename: "code-quartet-1.epub",
        title: "Quartet I: Lexer",
        authors: &["Niklaus Wirth"],
        publisher: Some("Verlag Algorithmus"),
        published: Some("1976-04-01"),
        language: "de",
        series: Some("Code Quartet"),
        series_index: Some("1"),
        has_cover: true,
    },
    Expected {
        filename: "code-quartet-2.epub",
        title: "Quartet II: Parser",
        authors: &["Niklaus Wirth"],
        publisher: Some("Verlag Algorithmus"),
        published: Some("1977-08-12"),
        language: "de",
        series: Some("Code Quartet"),
        series_index: Some("2"),
        has_cover: true,
    },
    Expected {
        filename: "code-quartet-3.epub",
        title: "Quartet III: Type Checker",
        authors: &["Niklaus Wirth", "Per Brinch Hansen"],
        publisher: Some("Verlag Algorithmus"),
        published: Some("1978-09-10"),
        language: "en",
        series: Some("Code Quartet"),
        series_index: Some("3"),
        has_cover: false,
    },
    Expected {
        filename: "code-quartet-4.epub",
        title: "Quartet IV: Codegen",
        authors: &["Niklaus Wirth"],
        publisher: Some("Verlag Algorithmus"),
        published: Some("1979-12-01"),
        language: "en",
        series: Some("Code Quartet"),
        series_index: Some("4"),
        has_cover: true,
    },
    Expected {
        filename: "polyglot-1.epub",
        title: "Polyglot Tales: Recits",
        authors: &["Evariste Galois"],
        publisher: Some("Maison Polyglotte"),
        published: Some("1830-05-29"),
        language: "fr",
        series: Some("Polyglot Tales"),
        series_index: Some("1"),
        has_cover: true,
    },
    Expected {
        filename: "polyglot-2.epub",
        title: "Polyglot Tales: Cuentos",
        authors: &["Jorge Luis Borges"],
        publisher: Some("Maison Polyglotte"),
        published: Some("1944-06-01"),
        language: "es",
        series: Some("Polyglot Tales"),
        series_index: Some("2"),
        has_cover: true,
    },
    Expected {
        filename: "polyglot-3.epub",
        title: "Polyglot Tales: Monogatari",
        authors: &["Soseki Natsume"],
        publisher: Some("Maison Polyglotte"),
        published: Some("1914-04-20"),
        language: "ja",
        series: Some("Polyglot Tales"),
        series_index: Some("3"),
        has_cover: true,
    },
    Expected {
        filename: "compiler-compendium-1.epub",
        title: "Compendium of Compilers I",
        authors: &["Alfred Aho", "Jeffrey Ullman"],
        publisher: Some("MIT Press Mirror"),
        published: Some("1977-01-01"),
        language: "en",
        series: Some("Compiler Compendium"),
        series_index: Some("1"),
        has_cover: true,
    },
    Expected {
        filename: "compiler-compendium-2.epub",
        title: "Compendium of Compilers II",
        authors: &["Alfred Aho", "Monica Lam"],
        publisher: Some("MIT Press Mirror"),
        published: Some("1986-10-15"),
        language: "en",
        series: Some("Compiler Compendium"),
        series_index: Some("2"),
        has_cover: true,
    },
    Expected {
        filename: "compiler-compendium-3.epub",
        title: "Compendium of Compilers III",
        authors: &["Alfred Aho", "Ravi Sethi", "Jeffrey Ullman"],
        publisher: Some("MIT Press Mirror"),
        published: Some("2006-08-31"),
        language: "en",
        series: Some("Compiler Compendium"),
        series_index: Some("3"),
        has_cover: false,
    },
    Expected {
        filename: "mathematica-minor-1.epub",
        title: "Minor Lemmas I",
        authors: &["Emmy Noether"],
        publisher: Some("Klein Mathematik"),
        published: Some("1918-07-23"),
        language: "de",
        series: Some("Mathematica Minor"),
        series_index: Some("1"),
        has_cover: true,
    },
    Expected {
        filename: "mathematica-minor-2.epub",
        title: "Minor Lemmas II",
        authors: &["Sophie Germain"],
        publisher: Some("Klein Mathematik"),
        published: Some("1815-01-08"),
        language: "fr",
        series: Some("Mathematica Minor"),
        series_index: Some("2"),
        has_cover: true,
    },
    Expected {
        filename: "mathematica-minor-3.epub",
        title: "Minor Lemmas III",
        authors: &["Sofya Kovalevskaya"],
        publisher: Some("Klein Mathematik"),
        published: Some("1888-12-24"),
        language: "en",
        series: Some("Mathematica Minor"),
        series_index: Some("3"),
        has_cover: false,
    },
    Expected {
        filename: "standalone-island.epub",
        title: "The Isle of Functions",
        authors: &["Haskell Curry"],
        publisher: Some("Lambda Books"),
        published: Some("1934-09-12"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "standalone-garden.epub",
        title: "The Garden of Closures",
        authors: &["Alonzo Church"],
        publisher: Some("Lambda Books"),
        published: Some("1936-04-19"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "standalone-river.epub",
        title: "Rio de Monadas",
        authors: &["Maria Zambrano"],
        publisher: Some("Editorial Omnibus"),
        published: Some("1957-03-03"),
        language: "es",
        series: None,
        series_index: None,
        has_cover: false,
    },
    Expected {
        filename: "standalone-mountain.epub",
        title: "Berg der Beweise",
        authors: &["David Hilbert"],
        publisher: Some("Klein Mathematik"),
        published: Some("1900-08-08"),
        language: "de",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "standalone-forest.epub",
        title: "La Foret des Algorithmes",
        authors: &["Henri Poincare"],
        publisher: Some("Maison Polyglotte"),
        published: Some("1899-11-17"),
        language: "fr",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "standalone-ocean.epub",
        title: "Ocean of Bytes",
        authors: &["Donald Knuth", "Leslie Lamport"],
        publisher: Some("MIT Press Mirror"),
        published: Some("1989-06-21"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "standalone-desert.epub",
        title: "Desert Protocols",
        authors: &["Vint Cerf", "Bob Kahn"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1974-05-10"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: false,
    },
    Expected {
        filename: "standalone-meadow.epub",
        title: "Meadow of Bits",
        authors: &["Claude Shannon"],
        publisher: Some("Omnibus Test Press"),
        published: Some("1948-07-30"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: true,
    },
    Expected {
        filename: "standalone-canyon.epub",
        title: "Canyon Echoes",
        authors: &["Annie Easley"],
        publisher: Some("Lambda Books"),
        published: Some("1955-04-23"),
        language: "en",
        series: None,
        series_index: None,
        has_cover: true,
    },
];

#[test]
fn fixture_epubs_parse_with_expected_metadata() {
    let dir = fixtures_dir();
    let result = omnibus_db::ebook::scan_ebook_library(Some(dir.to_str().unwrap()));
    assert!(result.error.is_none(), "scan errored: {:?}", result.error);
    assert_eq!(
        result.books.len(),
        EXPECTED.len(),
        "fixture count drifted: scanner found {}, EXPECTED has {}",
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
            m.publisher.as_deref(),
            exp.publisher,
            "{} publisher",
            exp.filename,
        );
        assert_eq!(
            m.published.as_deref(),
            exp.published,
            "{} published",
            exp.filename,
        );
        assert_eq!(
            m.language.as_deref(),
            Some(exp.language),
            "{} language",
            exp.filename,
        );
        assert_eq!(m.series.as_deref(), exp.series, "{} series", exp.filename);
        assert_eq!(
            m.series_index.as_deref(),
            exp.series_index,
            "{} series_index",
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
