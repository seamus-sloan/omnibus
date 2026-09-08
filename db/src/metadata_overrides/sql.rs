//! SQL fragments that read the override layer from inside a query.
//!
//! The Rust read path merges overrides in `upsert::apply_overrides`; these are
//! its mirror for the queries that must *sort*, *group* or *rank* on the
//! displayed value rather than the scanned one. Both sides have to agree —
//! a list ordered by the scanned title and rendered with the overridden one
//! is a list in no order at all — so the precedence gate below is written to
//! match `apply_overrides`' case for case.
//!
//! Every fragment here assumes the querying statement joins `books b`,
//! `scan_roots l` (on `b.library_id`) and `metadata_overrides mo` (on
//! `b.uuid`) — see [`OVERRIDE_JOIN`].

/// The two joins every fragment in this module reads from. A macro rather
/// than a `const` so it composes inside `concat!` with the fragments below.
///
/// `LEFT` on both: a book with no overrides row is the common case, and a
/// query that ranked only the overridden books would answer a different
/// question entirely.
macro_rules! override_join_sql {
    () => {
        " LEFT JOIN scan_roots l ON l.id = b.library_id \
          LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid "
    };
}

/// SQL mirror of `apply_overrides`' precedence gate: does this book's scan
/// root rank `omnibus_overrides` above `embedded_tags`? The stored list is
/// validated whole on write, so a token's byte offset is its rank; a list
/// carrying neither token falls back to overrides-win, as the Rust side does.
///
/// `COALESCE` on the column for the same reason: `merge_overrides_into_books`
/// falls back to `DEFAULT_METADATA_PRECEDENCE` when a book has no precedence
/// of its own, and an unjoined `scan_roots` row must not quietly drop the
/// override instead.
macro_rules! overrides_win_sql {
    () => {
        "(instr(COALESCE(l.metadata_precedence, ''), '\"omnibus_overrides\"') = 0
           OR instr(COALESCE(l.metadata_precedence, ''), '\"embedded_tags\"') = 0
           OR instr(COALESCE(l.metadata_precedence, ''), '\"omnibus_overrides\"')
              > instr(COALESCE(l.metadata_precedence, ''), '\"embedded_tags\"'))"
    };
}

/// The user-facing value of one override field, or NULL when the book has no
/// override for it (or its scan root ranks the override below the scan).
macro_rules! override_sql {
    ($path:literal) => {
        concat!(
            "NULLIF(CASE WHEN ",
            overrides_win_sql!(),
            " THEN json_extract(mo.overrides, '",
            $path,
            "') END, '')"
        )
    };
}

/// An axis keyed on the *displayed* value: the override where one exists, the
/// scanned column otherwise. `COLLATE NOCASE` is restated because a `COALESCE`
/// expression carries no implicit collation — without it the text axes would
/// silently become case-sensitive, unlike the NOCASE columns they wrap.
macro_rules! effective_text_sql {
    ($($path:literal),+ ; $scanned:literal) => {
        concat!("COALESCE(", $(override_sql!($path), ", ",)+ $scanned, ") COLLATE NOCASE")
    };
}

pub(crate) use {effective_text_sql, override_join_sql, override_sql, overrides_win_sql};
