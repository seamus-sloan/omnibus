//! Reindex-resurrection guard (#964): shared read-side of the
//! `entity_aliases` ledger that `cleanup::entity_ops::write_entity_alias`
//! writes when a library-cleanup merge absorbs an author, series, or tag.
//! Consulted by every taxonomy resolver *before* it mints a new row, so a
//! later reindex re-parsing a file that still names the merged-away entity
//! resolves back to the surviving canonical id instead of recreating the
//! row the merge just removed.

use std::collections::HashMap;

use omnibus_shared::CleanupKind;
use sqlx::Transaction;

#[cfg(test)]
mod tests;

/// Batched alias lookup: for each of `names`, report the canonical id it
/// now resolves to, if any prior merge recorded it as an alias. A name
/// absent from the returned map has never been merged away — the caller
/// falls through to its normal insert-or-resolve path. One `SELECT ...
/// WHERE alias_name IN (...)` per chunk rather than a query per name;
/// chunked at 500 to stay under SQLite's bound-parameter cap alongside the
/// `kind` bind.
///
/// The match is `COLLATE NOCASE` (ASCII case-fold, matching SQLite's builtin
/// collation and the `authors`/`series`/`tags` columns themselves — migration
/// `0002`) so a reindex that yields a casing variant of a merged-away name
/// still resolves to the canonical id rather than resurrecting it. Because
/// the stored `alias_name` may carry different casing than the queried
/// `names`, the returned map is keyed by the *input* name (matched via
/// `eq_ignore_ascii_case`, mirroring SQLite's own fold) rather than the row's
/// casing, so callers can look it up with the exact string they queried
/// with.
pub(crate) async fn resolve_entity_aliases(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    kind: CleanupKind,
    names: &[&str],
) -> Result<HashMap<String, i64>, sqlx::Error> {
    let mut out = HashMap::new();
    if names.is_empty() {
        return Ok(out);
    }
    for chunk in names.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT alias_name, canonical_id FROM entity_aliases \
             WHERE kind = ? AND alias_name COLLATE NOCASE IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, i64)>(&sql).bind(kind.as_str());
        for name in chunk {
            q = q.bind(*name);
        }
        for (alias_name, canonical_id) in q.fetch_all(&mut **tx).await? {
            for name in chunk.iter().filter(|n| n.eq_ignore_ascii_case(&alias_name)) {
                out.insert((*name).to_string(), canonical_id);
            }
        }
    }
    Ok(out)
}
