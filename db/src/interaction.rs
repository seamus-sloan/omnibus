//! The SQL expression behind the "Recently Interacted" sort axis: the most
//! recent moment *anyone* touched a book. Derived at read time from the
//! tables that already record each signal, so there is no stored column for
//! a write path to forget to bump.

/// Every interaction signal, folded into one INTEGER unix-seconds expression
/// over the `books` alias `b`. Expands to a string literal so the callers
/// that build a `const` projection can `concat!` it.
///
/// Two of the seven signals need no join: `save_overrides` and the cover
/// upsert both call `touch_book_last_modified`, so a metadata edit and a
/// cover replacement are already `books.last_modified`.
///
/// Journal drafts (migration `0040`) are excluded deliberately — a private
/// draft must not reveal its existence by moving a book up a shared sort.
///
/// The `NULLIF(…, 0)` maps "no signal at all" back to `NULL`, which SQLite
/// orders first ascending and last descending — the same placement the other
/// nullable axes get, and the one `page`'s keyset predicate assumes.
macro_rules! interacted_at_epoch_sql {
    () => {
        r"NULLIF(MAX(
        COALESCE(b.last_modified, 0),
        COALESCE(b.timestamp, 0),
        COALESCE((SELECT MAX(ur.updated_at) FROM user_ratings ur
                   WHERE ur.book_uuid = b.uuid), 0),
        COALESCE((SELECT MAX(MAX(je.created_at, je.updated_at)) FROM journal_entries je
                   WHERE je.book_uuid = b.uuid AND je.status = 'published'), 0),
        COALESCE((SELECT MAX(brs.updated_at) FROM book_read_status brs
                   WHERE brs.book_uuid = b.uuid), 0),
        COALESCE((SELECT MAX(pc.checked_in_at) FROM physical_copies pc
                   WHERE pc.book_uuid = b.uuid), 0)
    ), 0)"
    };
}
pub(crate) use interacted_at_epoch_sql;

/// [`interacted_at_epoch_sql`] formatted to fixed-width ISO, for the read
/// paths that round-trip the value as text: the wire field and the landing
/// cursor, which compares its primary axis lexicographically (ISO sorts
/// identically to chronological). `NULL` in yields `NULL` out.
macro_rules! interacted_at_iso_sql {
    () => {
        concat!(
            "strftime('%Y-%m-%dT%H:%M:%SZ', ",
            $crate::interaction::interacted_at_epoch_sql!(),
            ", 'unixepoch')"
        )
    };
}
pub(crate) use interacted_at_iso_sql;

/// Runtime handle on [`interacted_at_epoch_sql`], for `format!`-built queries
/// that order by the raw integer rather than projecting it.
pub(crate) const INTERACTED_AT_EPOCH: &str = interacted_at_epoch_sql!();

/// Runtime handle on [`interacted_at_iso_sql`].
pub(crate) const INTERACTED_AT_ISO: &str = interacted_at_iso_sql!();

#[cfg(test)]
mod tests;
