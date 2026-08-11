//! Library-cleanup detection: five domain-specific algorithms that surface
//! near-duplicate authors/series/tags, semicolon-soup tags, filename cruft
//! in book titles, and junk-author rows left behind by a third-party
//! import. Detection only — the persisted `dedup_suggestions` queue and
//! the apply/undo primitives are separate follow-up modules.

use std::collections::{HashMap, HashSet};

use omnibus_shared::{CleanupAction, CleanupKind};
use regex::Regex;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::normalize::normalize_author;

#[cfg(test)]
mod tests;

/// Errors returned by the cleanup detectors.
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("invalid detector pattern: {0}")]
    Pattern(#[from] regex::Error),
}

/// Confidence tier of a detected suggestion, per F5.9's technical design:
/// Tier 0 is a high-confidence normalized-key or regex match; Tier 1 is a
/// fuzzy match, always surfaced (never gated behind an opt-in toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Zero,
    One,
}

/// Enough information to act on a detected suggestion later: what the apply
/// primitive would do, keyed by [`CleanupAction`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CleanupPayload {
    /// Combine `source_ids` into `canonical_id` (author/series/tag merges).
    Merge {
        source_ids: Vec<i64>,
        source_names: Vec<String>,
        canonical_id: i64,
        canonical_name: String,
    },
    /// Replace tag `source_id` with the listed `atoms`, split on `delimiter`.
    Split {
        source_id: i64,
        source_name: String,
        atoms: Vec<String>,
        delimiter: String,
    },
    /// Adopt `proposed_title` for `book_id` in place of `current_title`.
    Rename {
        book_id: i64,
        book_uuid: String,
        current_title: String,
        proposed_title: String,
    },
    /// Remove `entity_id` outright (junk-author rows).
    Delete { entity_id: i64, name: String },
}

/// One detected cleanup opportunity, ready to persist as a
/// `dedup_suggestions` row (`kind`/`action`/[`CleanupPayload`]) once the
/// apply layer lands.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DetectedSuggestion {
    pub kind: CleanupKind,
    pub action: CleanupAction,
    pub tier: Tier,
    /// 1.0 for an exact Tier-0 match; the similarity score for a fuzzy
    /// Tier-1 match (`>= FUZZY_JACCARD_THRESHOLD`).
    pub score: f64,
    /// The entity that survives a merge, or the single entity a
    /// split/rename/delete acts on.
    pub primary_name: String,
    /// The other name in a two-way merge, or the proposed title for a
    /// rename. `None` when the suggestion has no single "other" name.
    pub secondary_name: Option<String>,
    /// How many *distinct* library books are affected if the suggestion is
    /// applied — a merge group's book is counted once even if it's linked
    /// to more than one of the group's members.
    pub book_count: i64,
    pub payload: CleanupPayload,
}

// ---------------------------------------------------------------------------
// Public detectors
// ---------------------------------------------------------------------------

/// Detect duplicate authors (normalized-key + fuzzy merges) plus junk-author
/// rows left behind by third-party tooling (Calibre/Smashwords artifacts).
pub async fn detect_authors(pool: &SqlitePool) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    let mut suggestions = detect_merges(pool, MergeEntity::Author).await?;
    suggestions.extend(detect_junk_authors(pool).await?);
    Ok(suggestions)
}

/// Detect duplicate series via the same normalized-key + fuzzy shape as
/// [`detect_authors`].
pub async fn detect_series(pool: &SqlitePool) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    detect_merges(pool, MergeEntity::Series).await
}

/// Detect duplicate tags via the same normalized-key + fuzzy shape as
/// [`detect_authors`].
pub async fn detect_tags_merge(pool: &SqlitePool) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    detect_merges(pool, MergeEntity::Tag).await
}

/// Detect tags that should be split into atoms: a `;`-delimited soup
/// (Tier 0) or a long name with embedded commas (Tier 1).
pub async fn detect_tags_split(pool: &SqlitePool) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    let rows = fetch_entity_rows(pool, MergeEntity::Tag).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let (tier, delimiter, atoms) = split_candidate(&row.name)?;
            Some(tag_split_suggestion(row, tier, delimiter, atoms))
        })
        .collect())
}

/// Detect book titles carrying filename cruft: an author/series/index
/// prefix (Tier 0) or a trailing parenthetical edition note (Tier 1).
pub async fn detect_book_titles(
    pool: &SqlitePool,
) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    let prefixes = compile_all(TITLE_PREFIX_PATTERNS)?;
    let suffix = Regex::new(TITLE_SUFFIX_PATTERN)?;

    // Sound prefilter: every pattern in TITLE_PREFIX_PATTERNS /
    // TITLE_SUFFIX_PATTERN requires at least one of these five literal
    // characters to match at all (','+'-' for the author-dash prefix,
    // '['/']' for the series-bracket prefix, '#' for the series-hash
    // prefix, '-' for the acronym-index prefix, '('/')' for the trailing
    // parenthetical) — a title with none of them cannot match any pattern,
    // so skipping it here can never miss a real suggestion. Cuts the
    // regex work (and the row transfer) down from every book to only the
    // ones that could plausibly match.
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, uuid, title FROM books
          WHERE instr(title, ',') > 0
             OR instr(title, '-') > 0
             OR instr(title, '[') > 0
             OR instr(title, '#') > 0
             OR instr(title, '(') > 0",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(id, uuid, title)| {
            let (tier, proposed) = strip_title_cruft(&title, &prefixes, &suffix)?;
            Some(book_title_suggestion(id, uuid, title, proposed, tier))
        })
        .collect())
}

/// Run every detector and return their suggestions concatenated. Each
/// detector targets its own table independently, so a caller wanting only
/// one kind should call it directly instead of filtering this result.
pub async fn detect_all(pool: &SqlitePool) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    let mut suggestions = detect_authors(pool).await?;
    suggestions.extend(detect_series(pool).await?);
    suggestions.extend(detect_tags_merge(pool).await?);
    suggestions.extend(detect_tags_split(pool).await?);
    suggestions.extend(detect_book_titles(pool).await?);
    Ok(suggestions)
}

// ---------------------------------------------------------------------------
// Merge detection: authors / series / tags share one normalized-key +
// fuzzy-Jaccard shape, parameterized on which taxonomy table to read.
// ---------------------------------------------------------------------------

/// Which normalized taxonomy table a merge-detection pass targets.
#[derive(Debug, Clone, Copy)]
enum MergeEntity {
    Author,
    Series,
    Tag,
}

impl MergeEntity {
    fn kind(self) -> CleanupKind {
        match self {
            Self::Author => CleanupKind::Author,
            Self::Series => CleanupKind::Series,
            Self::Tag => CleanupKind::Tag,
        }
    }

    /// `(id, name, sort, book_id)` — one row per linked book, `NULL` book_id
    /// for an entity with none, `sort` NULL where the table has no such
    /// column (tags). Left un-aggregated (no `COUNT`/`GROUP BY`) so a merge
    /// group can later count *distinct* affected books instead of summing
    /// each entity's count, which would double-count a book linked to more
    /// than one of the group's members.
    fn sql(self) -> &'static str {
        match self {
            Self::Author => {
                "SELECT a.id, a.name, a.sort, bal.book AS book_id
                   FROM authors a
                   LEFT JOIN books_authors_link bal ON bal.author = a.id"
            }
            Self::Series => {
                "SELECT s.id, s.name, s.sort, bsl.book AS book_id
                   FROM series s
                   LEFT JOIN books_series_link bsl ON bsl.series = s.id"
            }
            Self::Tag => {
                "SELECT t.id, t.name, NULL AS sort, btl.book AS book_id
                   FROM tags t
                   LEFT JOIN books_tags_link btl ON btl.tag = t.id"
            }
        }
    }
}

#[derive(Debug)]
struct EntityRow {
    id: i64,
    name: String,
    sort: Option<String>,
    /// Ids of every book linked to this entity — a set (not a count) so a
    /// merge group can union several entities' books and count *distinct*
    /// rows rather than double-counting a book linked to more than one.
    book_ids: HashSet<i64>,
}

impl EntityRow {
    fn book_count(&self) -> i64 {
        self.book_ids.len() as i64
    }
}

/// Fetch every row of `entity`'s table, folding the one-row-per-linked-book
/// result of [`MergeEntity::sql`] into one [`EntityRow`] per distinct id.
async fn fetch_entity_rows(
    pool: &SqlitePool,
    entity: MergeEntity,
) -> Result<Vec<EntityRow>, CleanupError> {
    let raw: Vec<(i64, String, Option<String>, Option<i64>)> =
        sqlx::query_as(entity.sql()).fetch_all(pool).await?;
    let mut rows: HashMap<i64, EntityRow> = HashMap::new();
    for (id, name, sort, book_id) in raw {
        let row = rows.entry(id).or_insert_with(|| EntityRow {
            id,
            name,
            sort,
            book_ids: HashSet::new(),
        });
        if let Some(book_id) = book_id {
            row.book_ids.insert(book_id);
        }
    }
    Ok(rows.into_values().collect())
}

/// Shared merge-detection body: Tier-0 GROUP-BY on the normalized key, then
/// a Tier-1 fuzzy pass over whatever the Tier-0 grouping left as singletons.
async fn detect_merges(
    pool: &SqlitePool,
    entity: MergeEntity,
) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    let rows = fetch_entity_rows(pool, entity).await?;
    let kind = entity.kind();

    let mut buckets: HashMap<String, Vec<&EntityRow>> = HashMap::new();
    for row in &rows {
        if let Some(key) = tier0_key(&row.name, row.sort.as_deref()) {
            buckets.entry(key).or_default().push(row);
        }
    }

    let mut suggestions = Vec::new();
    let mut singles: Vec<(&str, &EntityRow)> = Vec::new();
    for (key, group) in &buckets {
        if group.len() >= 2 {
            if let Some(canonical) = pick_canonical(group) {
                suggestions.push(merge_suggestion(kind, Tier::Zero, 1.0, group, canonical));
            }
        } else if let [row] = group.as_slice() {
            singles.push((key.as_str(), row));
        }
    }

    suggestions.extend(fuzzy_merge_suggestions(kind, &singles));
    Ok(suggestions)
}

/// The Tier-0 GROUP-BY key for an author/series/tag row: `COALESCE(sort,
/// name)` folded through [`normalize_author`] — the same lowercase /
/// strip-punctuation / collapse-whitespace fold, plus the `"Last, First"`
/// comma swap, that already keys the cross-format auto-attach match.
/// `None` when nothing survives (e.g. a name of only punctuation).
fn tier0_key(name: &str, sort: Option<&str>) -> Option<String> {
    normalize_author(sort.unwrap_or(name))
}

/// Tier-1 pass: bucket the rows a Tier-0 key left as singletons by shared
/// first token (keeps the comparison sub-quadratic on large sets), then
/// emit a merge suggestion for every pair inside a bucket scoring at least
/// [`FUZZY_JACCARD_THRESHOLD`] on whole-token-set Jaccard.
fn fuzzy_merge_suggestions(
    kind: CleanupKind,
    singles: &[(&str, &EntityRow)],
) -> Vec<DetectedSuggestion> {
    // Each candidate's token set is built once here, up front, and reused
    // for every pairwise comparison its bucket runs below — rebuilding it
    // per comparison would redo the same split/collect work O(bucket_len)
    // times over.
    let mut first_token_buckets: HashMap<&str, Vec<(HashSet<&str>, &EntityRow)>> = HashMap::new();
    for &(key, row) in singles {
        let Some(first) = key.split_whitespace().next() else {
            continue;
        };
        first_token_buckets
            .entry(first)
            .or_default()
            .push((token_set(key), row));
    }

    let mut suggestions = Vec::new();
    for bucket in first_token_buckets.values() {
        for (i, (tokens_a, row_a)) in bucket.iter().enumerate() {
            for (tokens_b, row_b) in &bucket[i + 1..] {
                let score = jaccard(tokens_a, tokens_b);
                if score >= FUZZY_JACCARD_THRESHOLD {
                    let pair = [*row_a, *row_b];
                    if let Some(canonical) = pick_canonical(&pair) {
                        suggestions.push(merge_suggestion(
                            kind,
                            Tier::One,
                            score,
                            &pair,
                            canonical,
                        ));
                    }
                }
            }
        }
    }
    suggestions
}

/// Fuzzy-match threshold: token-set Jaccard >= 0.85 (F5.9 technical design).
const FUZZY_JACCARD_THRESHOLD: f64 = 0.85;

fn token_set(key: &str) -> HashSet<&str> {
    key.split_whitespace().collect()
}

/// Whole-token-set Jaccard similarity: `|A ∩ B| / |A ∪ B|`.
fn jaccard(a: &HashSet<&str>, b: &HashSet<&str>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// Choose which row in a merge group survives: most linked books, ties
/// broken toward the lower id (first inserted). `None` only if `group` is
/// empty, which callers never pass.
fn pick_canonical<'a>(group: &[&'a EntityRow]) -> Option<&'a EntityRow> {
    group.iter().copied().reduce(|best, row| {
        if row.book_count() > best.book_count()
            || (row.book_count() == best.book_count() && row.id < best.id)
        {
            row
        } else {
            best
        }
    })
}

fn merge_suggestion(
    kind: CleanupKind,
    tier: Tier,
    score: f64,
    group: &[&EntityRow],
    canonical: &EntityRow,
) -> DetectedSuggestion {
    let source_ids: Vec<i64> = group
        .iter()
        .filter(|r| r.id != canonical.id)
        .map(|r| r.id)
        .collect();
    let source_names: Vec<String> = group
        .iter()
        .filter(|r| r.id != canonical.id)
        .map(|r| r.name.clone())
        .collect();
    // Distinct union, not a sum: a book linked to more than one of the
    // group's members (common for tags, possible for messy author imports)
    // must be counted once, not once per member that links to it.
    let book_count = group
        .iter()
        .flat_map(|r| r.book_ids.iter())
        .collect::<HashSet<_>>()
        .len() as i64;
    DetectedSuggestion {
        kind,
        action: CleanupAction::Merge,
        tier,
        score,
        primary_name: canonical.name.clone(),
        secondary_name: source_names.first().cloned(),
        book_count,
        payload: CleanupPayload::Merge {
            source_ids,
            source_names,
            canonical_id: canonical.id,
            canonical_name: canonical.name.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// Junk-author delete detection
// ---------------------------------------------------------------------------

/// Tooling-artifact author names left behind by a Calibre/Smashwords import
/// — e.g. `calibre (0.7.23) [http://calibre-ebook.com]`.
const JUNK_AUTHOR_PATTERNS: &[&str] = &[r"^calibre \(", r"\[http", r"Smashwords, Inc\."];

async fn detect_junk_authors(pool: &SqlitePool) -> Result<Vec<DetectedSuggestion>, CleanupError> {
    let patterns = compile_all(JUNK_AUTHOR_PATTERNS)?;
    let rows = fetch_entity_rows(pool, MergeEntity::Author).await?;
    Ok(rows
        .into_iter()
        .filter(|row| patterns.iter().any(|re| re.is_match(&row.name)))
        .map(|row| DetectedSuggestion {
            kind: CleanupKind::Author,
            action: CleanupAction::Delete,
            tier: Tier::Zero,
            score: 1.0,
            primary_name: row.name.clone(),
            secondary_name: None,
            book_count: row.book_count(),
            payload: CleanupPayload::Delete {
                entity_id: row.id,
                name: row.name,
            },
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Tag split detection
// ---------------------------------------------------------------------------

/// A tag name at or above this length is a candidate for the Tier-1
/// embedded-comma split heuristic.
const TAG_SPLIT_LONG_LEN: usize = 30;

/// Detect a tag name that should be split into atoms: a `;`-delimited soup
/// (Tier 0) or a long name with embedded commas (Tier 1). Returns
/// `(tier, delimiter, atoms)`; `None` when `name` doesn't look splittable.
fn split_candidate(name: &str) -> Option<(Tier, &'static str, Vec<String>)> {
    if name.contains(';') {
        let atoms = split_trim(name, ';');
        if atoms.len() >= 2 {
            return Some((Tier::Zero, ";", atoms));
        }
    }
    if name.len() >= TAG_SPLIT_LONG_LEN && name.contains(',') {
        let atoms = split_trim(name, ',');
        if atoms.len() >= 2 {
            return Some((Tier::One, ",", atoms));
        }
    }
    None
}

fn split_trim(name: &str, delimiter: char) -> Vec<String> {
    name.split(delimiter)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn tag_split_suggestion(
    row: EntityRow,
    tier: Tier,
    delimiter: &'static str,
    atoms: Vec<String>,
) -> DetectedSuggestion {
    let score = if tier == Tier::Zero { 1.0 } else { 0.75 };
    DetectedSuggestion {
        kind: CleanupKind::Tag,
        action: CleanupAction::Split,
        tier,
        score,
        primary_name: row.name.clone(),
        secondary_name: None,
        book_count: row.book_count(),
        payload: CleanupPayload::Split {
            source_id: row.id,
            source_name: row.name,
            atoms,
            delimiter: delimiter.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Book-title cruft detection
// ---------------------------------------------------------------------------

/// Leading-prefix patterns stripped from a book title, tried in order (the
/// first match wins). All Tier 0 — unambiguous filename artifacts.
const TITLE_PREFIX_PATTERNS: &[&str] = &[
    r"^[^,]+,\s*[^-]+-\s*",       // "Maas, Sarah J - "
    r"^\[[^\]]+\s+\d+\]\s*",      // "[Mistborn 01] "
    r"^[\w .]+#\d+\s*",           // "Mistborn #1 "
    r"^[A-Za-z]{2,8}\d{1,3}-\s*", // "ToG04-"
];

/// Trailing parenthetical (edition/series note) stripped from a book
/// title. Tier 1 — lower confidence than the leading-cruft patterns
/// because a legitimate title can end in a parenthetical.
const TITLE_SUFFIX_PATTERN: &str = r"\s*\([^()]*\)\s*$";

/// Apply the leading-prefix (Tier 0) and trailing-parenthetical (Tier 1)
/// patterns to `title`. Returns the winning tier and the stripped title, or
/// `None` if neither pattern matched or the result is empty/unchanged.
fn strip_title_cruft(title: &str, prefixes: &[Regex], suffix: &Regex) -> Option<(Tier, String)> {
    let mut stripped = title;
    let mut tier0_hit = false;
    for re in prefixes {
        if let Some(m) = re.find(stripped) {
            stripped = &stripped[m.end()..];
            tier0_hit = true;
            break;
        }
    }
    let tier1_hit = if let Some(m) = suffix.find(stripped) {
        stripped = &stripped[..m.start()];
        true
    } else {
        false
    };
    let proposed = stripped.trim();
    if proposed.is_empty() || proposed == title || (!tier0_hit && !tier1_hit) {
        return None;
    }
    Some((
        if tier0_hit { Tier::Zero } else { Tier::One },
        proposed.to_string(),
    ))
}

fn book_title_suggestion(
    id: i64,
    uuid: String,
    title: String,
    proposed: String,
    tier: Tier,
) -> DetectedSuggestion {
    let score = if tier == Tier::Zero { 1.0 } else { 0.75 };
    DetectedSuggestion {
        kind: CleanupKind::BookTitle,
        action: CleanupAction::Rename,
        tier,
        score,
        primary_name: title.clone(),
        secondary_name: Some(proposed.clone()),
        book_count: 1,
        payload: CleanupPayload::Rename {
            book_id: id,
            book_uuid: uuid,
            current_title: title,
            proposed_title: proposed,
        },
    }
}

fn compile_all(patterns: &[&str]) -> Result<Vec<Regex>, regex::Error> {
    patterns.iter().map(|p| Regex::new(p)).collect()
}
