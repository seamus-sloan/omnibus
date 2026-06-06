//! FTS5 books path for the search palette: BM25-ranked title/author/series
//! matches with override-aware overlays applied after hydration so the
//! palette row matches what the rest of the app renders.

use sqlx::{Row, SqlitePool};

use omnibus_shared::PaletteBookHit;

use crate::books::parse_json_array;
use crate::helpers::build_fts_match;
use crate::metadata_overrides::load_overrides_bulk;

use super::PaletteError;

/// Run the FTS5 books arm of the palette for `trimmed` (already-trimmed,
/// length-capped query) scoped to `library_path`, capped to `limit`.
/// Returns an empty vec when `build_fts_match` can't produce a MATCH
/// expression for the input.
pub async fn search_books(
    pool: &SqlitePool,
    library_path: &str,
    trimmed: &str,
    limit: i32,
) -> Result<Vec<PaletteBookHit>, PaletteError> {
    let Some(match_expr) = build_fts_match(trimmed) else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(
        r#"
        SELECT b.id, b.uuid, b.title, b.has_cover, b.accent_color,
               SUBSTR(b.pubdate, 1, 4) AS year,

               (SELECT GROUP_CONCAT(a.name, ', ')
                  FROM (SELECT a2.name FROM books_authors_link bal
                          JOIN authors a2 ON a2.id = bal.author
                         WHERE bal.book = b.id
                         ORDER BY bal.position) a)          AS author_display,

               (SELECT json_group_array(format)
                  FROM (SELECT format FROM book_files
                         WHERE book_id = b.id
                         ORDER BY format))                  AS formats_json

        FROM books_fts
        JOIN books b ON b.id = books_fts.rowid
        JOIN libraries l ON l.id = b.library_id
        WHERE books_fts MATCH ?1 AND l.path = ?2
        ORDER BY bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0), b.sort, b.id
        LIMIT ?3
        "#,
    )
    .bind(&match_expr)
    .bind(library_path)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut uuids: Vec<String> = Vec::with_capacity(rows.len());
    let mut hits: Vec<PaletteBookHit> = Vec::with_capacity(rows.len());
    for r in rows.iter() {
        let id: i64 = r.get("id");
        let uuid: String = r.get("uuid");
        let has_cover: i64 = r.get("has_cover");
        uuids.push(uuid.clone());
        hits.push(PaletteBookHit {
            id,
            uuid: uuid.clone(),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            author_display: r
                .get::<Option<String>, _>("author_display")
                .unwrap_or_default(),
            year: r.get("year"),
            formats: parse_json_array(r.get("formats_json"))?,
            cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
            accent: r.get("accent_color"),
        });
    }

    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for (hit, uuid) in hits.iter_mut().zip(uuids.iter()) {
        if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
            if let Some(ref t) = ov.title {
                hit.title = t.clone();
            }
            if let Some(ref creators) = ov.creators {
                hit.author_display = creators
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            // Mirror `apply_overrides`: surface user-uploaded covers even
            // when the scanned book had `has_cover = 0`.
            if *has_cover_ov {
                hit.cover_url = Some(format!("/api/covers/{}", hit.uuid));
            }
        }
    }

    Ok(hits)
}
