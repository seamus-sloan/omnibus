//! Tests for title/author normalization: casefolding, punctuation
//! collapsing, `&` expansion, diacritic folding, last/first-name swapping,
//! idempotence and panic-safety over varied Unicode input, and the `_norm`
//! column boot backfill's error propagation.

use super::*;
use crate::pool::init_db;

#[test]
fn normalize_title_casefolds_and_collapses_punctuation() {
    assert_eq!(normalize_title("Dracula"), Some("dracula".into()));
    assert_eq!(normalize_title("  DRACULA!  "), Some("dracula".into()));
    assert_eq!(
        normalize_title("The Hitch-Hiker's Guide"),
        Some("the hitch hiker s guide".into())
    );
    assert_eq!(
        normalize_title("Dune:  Messiah"),
        Some("dune messiah".into())
    );
}

#[test]
fn normalize_title_expands_ampersand_to_and() {
    assert_eq!(
        normalize_title("A Tale of Mirth & Magic"),
        Some("a tale of mirth and magic".into())
    );
    assert_eq!(
        normalize_title("A Tale of Mirth & Magic"),
        normalize_title("A Tale of Mirth and Magic"),
        "the two spellings must converge on one key"
    );
}

#[test]
fn normalize_expands_ampersand_glued_to_its_neighbours() {
    // The decision behind #1789: `&` is only ever the word "and", so it
    // expands in every position rather than only between spaces. "R&B"
    // therefore keys as `r and b`. That costs nothing — two writers of "R&B"
    // still agree — and it makes "R and B" agree with them too.
    assert_eq!(
        normalize_title("R&B Classics"),
        Some("r and b classics".into())
    );
    assert_eq!(
        normalize_title("R&B Classics"),
        normalize_title("R and B Classics")
    );
    assert_eq!(
        normalize_author("Wilkie & Sons"),
        Some("wilkie and sons".into())
    );
    // Leading/trailing ampersands keep the folder's no-edge-space invariant.
    assert_eq!(normalize_title("& Sons"), Some("and sons".into()));
    assert_eq!(normalize_title("Sons &"), Some("sons and".into()));
}

#[test]
fn normalize_title_leaves_plus_collapsed_to_a_space() {
    // `+` gets no expansion: it is a literal part of a token far more often
    // than a conjunction, so expanding it would key "C++ Primer" as
    // `c and and primer`. It stays a separator, identically on both sides.
    assert_eq!(normalize_title("C++ Primer"), Some("c primer".into()));
    assert_eq!(normalize_title("Milk + Honey"), Some("milk honey".into()));
}

#[test]
fn normalize_title_folds_diacritics() {
    assert_eq!(normalize_title("Çítåtelka"), Some("citatelka".into()));
    assert_eq!(normalize_title("Mémoires"), Some("memoires".into()));
}

#[test]
fn normalize_title_returns_none_when_nothing_survives() {
    assert_eq!(normalize_title(""), None);
    assert_eq!(normalize_title("  --- "), None);
}

#[test]
fn normalize_title_keeps_distinct_titles_distinct() {
    // The whole point of exact matching: no substring/fuzzy collapse.
    assert_ne!(normalize_title("Dune"), normalize_title("Dune Messiah"));
}

#[test]
fn normalize_author_swaps_single_comma_last_first() {
    assert_eq!(normalize_author("Stoker, Bram"), Some("bram stoker".into()));
    assert_eq!(normalize_author("Bram Stoker"), Some("bram stoker".into()));
    // Two commas: not a Last, First form — fold as-is.
    assert_eq!(
        normalize_author("Smith, John, Jr."),
        Some("smith john jr".into())
    );
}

/// Varied-Unicode inputs that stress every branch of the folder: accents,
/// mixed scripts, emoji, article prefixes, whitespace runs, empty/blank,
/// comma forms (author swap), CJK, RTL, and a very long string. Shared by
/// the idempotence and no-panic tests below.
fn tricky_inputs() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        "   ",
        "\t\n  \r",
        "---",
        "!!! ??? ...",
        "Dracula",
        "  DRACULA!  ",
        "The Hitch-Hiker's Guide",
        "A Tale of Two Cities",
        "An Anthology",
        "The",
        "Dune:  Messiah",
        "multiple     internal     spaces",
        "Çítåtelka",
        "Mémoires d'outre-tombe",
        "Ĳsselmeer œuvre æsthetic ﬁ ﬂ",
        "Stoker, Bram",
        "Smith, John, Jr.",
        "café ☕ résumé 📚 naïve",
        "🎉🎊✨",
        "Война и мир",
        "戦争と平和",
        "مرحبا بالعالم",
        "ᚠᚢᚦᚨᚱᚲ",
        "ﬀ ﬁ ﬂ ﬃ ﬄ",                         // ligatures that deunicode expands
        "①②③ ⅣⅤⅥ",                           // enclosed / roman numerals
        "\u{200B}zero\u{200B}width\u{200B}", // zero-width spaces
        "  ,  ",                             // comma with only blanks around it
        ",leading comma",
        "trailing comma,",
        "A Tale of Mirth & Magic", // ampersand expansion
        "R&B",                     // ampersand glued to both neighbours
        "&",                       // ampersand alone
        "& leading",
        "trailing &",
        "&&&",
        "C++ Primer", // plus left as a separator
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    // A very long input to exercise the capacity/allocation path.
    v.push("Tomás Ërník ".repeat(5_000));
    v
}

#[test]
fn normalize_title_is_idempotent_over_varied_unicode() {
    for input in tricky_inputs() {
        let once = normalize_title(&input);
        // Re-normalizing the output must be a fixed point: applying the
        // folder a second time yields exactly the first result.
        let twice = once.as_deref().and_then(normalize_title);
        assert_eq!(twice, once, "title not idempotent for {input:?}");
    }
}

#[test]
fn normalize_author_is_idempotent_over_varied_unicode() {
    for input in tricky_inputs() {
        let once = normalize_author(&input);
        let twice = once.as_deref().and_then(normalize_author);
        assert_eq!(twice, once, "author not idempotent for {input:?}");
    }
}

#[test]
fn normalize_never_panics_over_varied_unicode() {
    // The folder runs on arbitrary scanned metadata; a panic here would
    // abort a library scan. Assert it always returns without panicking and
    // that any surviving key is non-empty and free of leading/trailing/
    // doubled ASCII spaces (the folder's invariant).
    for input in tricky_inputs() {
        for key in [normalize_title(&input), normalize_author(&input)]
            .into_iter()
            .flatten()
        {
            assert!(!key.is_empty(), "empty key survived for {input:?}");
            assert!(!key.starts_with(' '), "leading space for {input:?}");
            assert!(!key.ends_with(' '), "trailing space for {input:?}");
            assert!(!key.contains("  "), "doubled space for {input:?}");
        }
    }
}

#[tokio::test]
async fn backfill_norm_columns_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = backfill_norm_columns(&pool).await.unwrap_err();
    assert!(matches!(err, NormalizeError::Db(_)));
}

#[tokio::test]
async fn backfill_norm_columns_preserves_author_norm_when_position_0_link_is_absent() {
    // `insert_author_links` skips a first creator on the `ignored_authors`
    // blocklist and leaves a positional gap rather than renumbering, while the
    // sync writer stores `author_norm` from `creators.first()` regardless. The
    // recompute therefore comes back non-derivable for such a book, and must
    // not overwrite the stored key with NULL — that key is the only copy.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, author_norm) \
         VALUES ('u1', 1, '', 'Drácula!', 'blocklisted quill') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    backfill_norm_columns(&pool).await.unwrap();

    let (title_norm, author_norm): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title_norm.as_deref(), Some("dracula"));
    assert_eq!(author_norm.as_deref(), Some("blocklisted quill"));
}

#[tokio::test]
async fn backfill_norm_columns_fills_only_null_rows_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('u1', 1, '', 'Drácula!') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO authors (name, sort) VALUES ('Stoker, Bram', 'Stoker, Bram')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, 1, 0)")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    backfill_norm_columns(&pool).await.unwrap();

    let (title_norm, author_norm): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title_norm.as_deref(), Some("dracula"));
    assert_eq!(author_norm.as_deref(), Some("bram stoker"));

    // A second run must not touch already-backfilled rows.
    sqlx::query("UPDATE books SET author_norm = 'sentinel' WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    backfill_norm_columns(&pool).await.unwrap();
    let (_, author_norm): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(author_norm.as_deref(), Some("sentinel"));
}
