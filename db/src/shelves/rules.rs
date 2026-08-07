//! Smart-rule → SQL predicate translation.
//!
//! Each [`ShelfRule`] becomes an `EXISTS`/comparison fragment over the `books b`
//! alias; the rule set combines with `OR` (match any) or `AND` (match all).
//! Text fields (tag/genre/author/series/format) match by **name**,
//! case-insensitively — equality via `COLLATE NOCASE`, substring/prefix via
//! `LIKE` (`contains` / `starts with`). Tag rules match the *effective*
//! (override-aware) tag set, not just `books_tags_link` — see
//! [`tag_condition`]; genre rules match the override-only genre list — see
//! [`genre_condition`]. Owner-scoped fields
//! (`rating`, `status`) bind the shelf owner's id: `status` matches the
//! owner's read state in `book_read_status`, treating a missing row as
//! `unread`.

use omnibus_shared::{MatchMode, ReadStatus, RuleField, RuleOp, ShelfRule};

use super::ShelfError;

/// A positional bind for a membership query. Owned so callers can `.bind()` by
/// value without lifetime gymnastics.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Bind {
    Text(String),
    Int(i64),
}

/// A WHERE fragment over the `books b` alias plus its ordered binds.
pub(super) struct Predicate {
    pub sql: String,
    pub binds: Vec<Bind>,
}

/// Combine `rules` under `match_mode` into one predicate, resolving owner-scoped
/// fields against `owner_id`. Empty `rules` matches nothing (`"0"`).
pub(super) fn membership_predicate(
    rules: &[ShelfRule],
    match_mode: MatchMode,
    owner_id: i64,
) -> Result<Predicate, ShelfError> {
    if rules.is_empty() {
        return Ok(Predicate {
            sql: "0".into(),
            binds: Vec::new(),
        });
    }
    let mut parts = Vec::with_capacity(rules.len());
    let mut binds = Vec::new();
    for rule in rules {
        let (sql, mut b) = condition_sql(rule, owner_id)?;
        parts.push(sql);
        binds.append(&mut b);
    }
    let joiner = match match_mode {
        MatchMode::Any => " OR ",
        MatchMode::All => " AND ",
    };
    Ok(Predicate {
        sql: format!("({})", parts.join(joiner)),
        binds,
    })
}

/// Translate one condition into `(sql, binds)`.
fn condition_sql(rule: &ShelfRule, owner_id: i64) -> Result<(String, Vec<Bind>), ShelfError> {
    // Central op gate: the SQL arms below only handle ops the field accepts.
    if !rule.field.accepts(rule.op) {
        return Err(unsupported(rule));
    }
    let v = rule.value.trim();
    match rule.field {
        // Text fields resolve against the normalized taxonomy `name` columns
        // (all `COLLATE NOCASE`), so the user types a name, not an id.
        RuleField::Tag => tag_condition(rule),
        RuleField::Genre => genre_condition(rule),
        RuleField::Author => text_condition(
            rule,
            "SELECT 1 FROM books_authors_link bal JOIN authors a ON a.id = bal.author \
             WHERE bal.book = b.id AND ",
            "a.name",
        ),
        RuleField::Series => text_condition(
            rule,
            "SELECT 1 FROM books_series_link bsl JOIN series s ON s.id = bsl.series \
             WHERE bsl.book = b.id AND ",
            "s.name",
        ),
        RuleField::Format => text_condition(
            rule,
            "SELECT 1 FROM book_files bf WHERE bf.book_id = b.id AND ",
            "bf.format",
        ),
        RuleField::Rating => {
            let cmp = match rule.op {
                RuleOp::Is => "=",
                RuleOp::Gte => ">=",
                _ => return Err(unsupported(rule)),
            };
            Ok((
                format!(
                    "EXISTS (SELECT 1 FROM user_ratings ur \
                     WHERE ur.book_uuid = b.uuid AND ur.user_id = ? AND ur.half_stars {cmp} ?)"
                ),
                vec![Bind::Int(owner_id), Bind::Int(parse_half_stars(v)?)],
            ))
        }
        RuleField::Year => {
            let cmp = match rule.op {
                RuleOp::Is => "=",
                RuleOp::Gte => ">=",
                _ => return Err(unsupported(rule)),
            };
            Ok((
                format!("CAST(substr(b.pubdate, 1, 4) AS INTEGER) {cmp} ?"),
                vec![Bind::Int(parse_id(v)?)],
            ))
        }
        RuleField::DateAdded | RuleField::DateUpdated => date_condition(rule, v),
        RuleField::Status => status_condition(v, owner_id),
    }
}

/// Read-state condition over the owner's `book_read_status` rows. `unread`
/// matches both an explicit `unread` row and the absence of any row (the
/// convention everywhere read state is consumed), so it is a `NOT EXISTS` over
/// the two "touched" states; `reading` / `finished` are a plain `EXISTS` on the
/// matching status. Only the `is` op reaches here (the field accepts no other).
fn status_condition(v: &str, owner_id: i64) -> Result<(String, Vec<Bind>), ShelfError> {
    let status = ReadStatus::from_str(v.trim())
        .ok_or_else(|| ShelfError::InvalidRule(format!("unknown read status {v:?}")))?;
    let sql = match status {
        ReadStatus::Unread => "NOT EXISTS (SELECT 1 FROM book_read_status rs \
             WHERE rs.book_uuid = b.uuid AND rs.user_id = ? \
               AND rs.status IN ('reading', 'finished'))"
            .to_string(),
        ReadStatus::Reading | ReadStatus::Finished => format!(
            "EXISTS (SELECT 1 FROM book_read_status rs \
             WHERE rs.book_uuid = b.uuid AND rs.user_id = ? AND rs.status = '{}')",
            status.as_str()
        ),
    };
    Ok((sql, vec![Bind::Int(owner_id)]))
}

/// Date-field conditions over `books.timestamp` / `books.last_modified`, which
/// are INTEGER unix-seconds (migration 0038): the relative-window comparison
/// stays numeric via `strftime('%s', …)`, and the absolute-date comparisons
/// render the column back to a calendar date with the `'unixepoch'` modifier.
fn date_condition(rule: &ShelfRule, v: &str) -> Result<(String, Vec<Bind>), ShelfError> {
    let col = match rule.field {
        RuleField::DateAdded => "b.timestamp",
        RuleField::DateUpdated => "b.last_modified",
        _ => unreachable!("date_condition called for non-date field"),
    };
    match rule.op {
        RuleOp::InLast => Ok((
            format!("{col} >= CAST(strftime('%s', 'now', ?) AS INTEGER)"),
            vec![Bind::Text(parse_relative_window(v)?)],
        )),
        RuleOp::Between => {
            let (start, end) = v.split_once("..").ok_or_else(|| {
                ShelfError::InvalidRule(format!("range must be START..END, got {v:?}"))
            })?;
            Ok((
                format!("date({col}, 'unixepoch') BETWEEN ? AND ?"),
                vec![
                    Bind::Text(validate_date(start)?),
                    Bind::Text(validate_date(end)?),
                ],
            ))
        }
        RuleOp::Before => Ok((
            format!("date({col}, 'unixepoch') < ?"),
            vec![Bind::Text(validate_date(v)?)],
        )),
        RuleOp::After => Ok((
            format!("date({col}, 'unixepoch') > ?"),
            vec![Bind::Text(validate_date(v)?)],
        )),
        _ => Err(unsupported(rule)),
    }
}

/// Build a case-insensitive `EXISTS`/`NOT EXISTS` text predicate for a joined
/// name column.
///
/// `inner` is the subquery up to (but not including) the column comparison, e.g.
/// `"SELECT 1 FROM books_authors_link bal JOIN authors a ON a.id = bal.author
/// WHERE bal.book = b.id AND "`; `col` is the compared column (`"a.name"`). Equality
/// (`is`/`is_not`/`includes`) uses `COLLATE NOCASE`; `contains`/`starts_with`
/// use `LIKE` (case-insensitive for ASCII) with metacharacters escaped so user
/// text matches literally.
fn text_condition(
    rule: &ShelfRule,
    inner: &str,
    col: &str,
) -> Result<(String, Vec<Bind>), ShelfError> {
    let v = rule.value.trim();
    let (cmp, bind, negate) = match rule.op {
        RuleOp::Is | RuleOp::Includes => (
            format!("{col} = ? COLLATE NOCASE"),
            Bind::Text(v.into()),
            false,
        ),
        RuleOp::IsNot => (
            format!("{col} = ? COLLATE NOCASE"),
            Bind::Text(v.into()),
            true,
        ),
        RuleOp::Contains => (
            format!("{col} LIKE ? ESCAPE '\\'"),
            Bind::Text(format!("%{}%", like_escape(v))),
            false,
        ),
        RuleOp::StartsWith => (
            format!("{col} LIKE ? ESCAPE '\\'"),
            Bind::Text(format!("{}%", like_escape(v))),
            false,
        ),
        _ => return Err(unsupported(rule)),
    };
    let exists = format!("EXISTS ({inner}{cmp})");
    let sql = if negate {
        format!("NOT {exists}")
    } else {
        exists
    };
    Ok((sql, vec![bind]))
}

/// Build a tag predicate over the book's *effective* tag set.
///
/// A `subjects` override replaces a book's scanned tags wholesale, and its
/// memberships live only in the override JSON — `materialize_tag_rows`
/// deliberately keeps them out of `books_tags_link` so revert-to-scanned
/// works. Mirroring `get_tag_cloud`'s membership CTE, a book matches through
/// exactly one arm: its canonical `books_tags_link` rows when it has no
/// subjects override, or `json_each` over the override array when it does.
/// A single-arm canonical predicate both misses override-added tags and
/// keeps matching scanned tags the override removed.
fn tag_condition(rule: &ShelfRule) -> Result<(String, Vec<Bind>), ShelfError> {
    let v = rule.value.trim();
    let (canonical_cmp, override_cmp, pattern, negate) = match rule.op {
        RuleOp::Is | RuleOp::IsNot => (
            "t.name = ? COLLATE NOCASE",
            "je.value = ? COLLATE NOCASE",
            v.to_string(),
            rule.op == RuleOp::IsNot,
        ),
        RuleOp::Contains => (
            "t.name LIKE ? ESCAPE '\\'",
            "je.value LIKE ? ESCAPE '\\'",
            format!("%{}%", like_escape(v)),
            false,
        ),
        RuleOp::StartsWith => (
            "t.name LIKE ? ESCAPE '\\'",
            "je.value LIKE ? ESCAPE '\\'",
            format!("{}%", like_escape(v)),
            false,
        ),
        _ => return Err(unsupported(rule)),
    };
    let matched = format!(
        "((NOT EXISTS (SELECT 1 FROM metadata_overrides mo \
           WHERE mo.book_uuid = b.uuid \
             AND json_type(mo.overrides, '$.subjects') IS NOT NULL) \
          AND EXISTS (SELECT 1 FROM books_tags_link btl \
           JOIN tags t ON t.id = btl.tag \
           WHERE btl.book = b.id AND {canonical_cmp})) \
         OR EXISTS (SELECT 1 FROM metadata_overrides mo \
           JOIN json_each(mo.overrides, '$.subjects') je \
           WHERE mo.book_uuid = b.uuid AND {override_cmp}))"
    );
    let sql = if negate {
        format!("NOT {matched}")
    } else {
        matched
    };
    Ok((sql, vec![Bind::Text(pattern.clone()), Bind::Text(pattern)]))
}

/// Build a genre predicate over the book's override-stored genre list.
///
/// Genres have no scan source and no link table (migration `0066`) — a
/// book's genres live solely in `metadata_overrides.overrides -> '$.genres'`.
/// So unlike [`tag_condition`]'s two membership arms, one `json_each` over
/// the override array is the whole effective set; a book with no override
/// (or no `$.genres` key) simply has no genres and never matches a positive
/// rule.
fn genre_condition(rule: &ShelfRule) -> Result<(String, Vec<Bind>), ShelfError> {
    let v = rule.value.trim();
    let (cmp, pattern, negate) = match rule.op {
        RuleOp::Is | RuleOp::IsNot => (
            "je.value = ? COLLATE NOCASE",
            v.to_string(),
            rule.op == RuleOp::IsNot,
        ),
        RuleOp::Contains => (
            "je.value LIKE ? ESCAPE '\\'",
            format!("%{}%", like_escape(v)),
            false,
        ),
        RuleOp::StartsWith => (
            "je.value LIKE ? ESCAPE '\\'",
            format!("{}%", like_escape(v)),
            false,
        ),
        _ => return Err(unsupported(rule)),
    };
    let exists = format!(
        "EXISTS (SELECT 1 FROM metadata_overrides mo \
         JOIN json_each(mo.overrides, '$.genres') je \
         WHERE mo.book_uuid = b.uuid AND {cmp})"
    );
    let sql = if negate {
        format!("NOT {exists}")
    } else {
        exists
    };
    Ok((sql, vec![Bind::Text(pattern)]))
}

/// Escape `LIKE` metacharacters (`\`, `%`, `_`) so user text matches literally.
/// Pairs with the `ESCAPE '\'` clause in [`text_condition`].
fn like_escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn parse_id(v: &str) -> Result<i64, ShelfError> {
    v.parse::<i64>()
        .map_err(|_| ShelfError::InvalidRule(format!("expected an integer, got {v:?}")))
}

/// Parse a whole/half star value (`"4"`, `"4.5"`) into a `1..=10` half-star count.
fn parse_half_stars(v: &str) -> Result<i64, ShelfError> {
    let stars: f64 = v
        .parse()
        .map_err(|_| ShelfError::InvalidRule(format!("invalid rating {v:?}")))?;
    if !(0.5..=5.0).contains(&stars) {
        return Err(ShelfError::InvalidRule(
            "rating must be between 0.5 and 5".into(),
        ));
    }
    // The 0.5..=5.0 range check above bounds the scaled value to 1..=10, so
    // the cast cannot truncate or saturate.
    #[allow(clippy::cast_possible_truncation)]
    let half_stars = (stars * 2.0).round() as i64;
    Ok(half_stars)
}

/// Parse a relative window (`"30d"`, `"2w"`, `"3m"`, `"1y"`) into a SQLite
/// datetime modifier (`"-30 days"`).
fn parse_relative_window(v: &str) -> Result<String, ShelfError> {
    let invalid = || ShelfError::InvalidRule(format!("invalid relative window {v:?}"));
    let (num, unit) = v.split_at(v.len().checked_sub(1).ok_or_else(invalid)?);
    let n: i64 = num.parse().map_err(|_| invalid())?;
    if n <= 0 {
        return Err(ShelfError::InvalidRule("window must be positive".into()));
    }
    let unit_word = match unit {
        "d" => "days",
        "w" => "weeks",
        "m" => "months",
        "y" => "years",
        _ => return Err(invalid()),
    };
    Ok(format!("-{n} {unit_word}"))
}

/// Validate an ISO `YYYY-MM-DD` date string, returning it trimmed.
fn validate_date(v: &str) -> Result<String, ShelfError> {
    let v = v.trim();
    let ok = v.len() == 10
        && v.as_bytes().iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !ok {
        return Err(ShelfError::InvalidRule(format!(
            "date must be YYYY-MM-DD, got {v:?}"
        )));
    }
    Ok(v.to_string())
}

fn unsupported(rule: &ShelfRule) -> ShelfError {
    ShelfError::InvalidRule(format!(
        "operator {} is not supported for field {}",
        rule.op.as_str(),
        rule.field.as_str()
    ))
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    fn rule(field: RuleField, op: RuleOp, value: &str) -> ShelfRule {
        ShelfRule {
            field,
            op,
            value: value.into(),
        }
    }

    #[test]
    fn membership_joins_with_or_for_any() {
        let p = membership_predicate(
            &[
                rule(RuleField::Tag, RuleOp::Is, "Fantasy"),
                rule(RuleField::Tag, RuleOp::Is, "Sci-fi"),
            ],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(p.sql.contains(" OR "));
        // Each tag rule binds its pattern twice: once for the canonical-link
        // arm, once for the override-JSON arm.
        assert_eq!(p.binds.len(), 4);
    }

    #[test]
    fn membership_joins_with_and_for_all() {
        let p = membership_predicate(
            &[
                rule(RuleField::Tag, RuleOp::Is, "Fantasy"),
                rule(RuleField::Author, RuleOp::Is, "7"),
            ],
            MatchMode::All,
            1,
        )
        .unwrap();
        assert!(p.sql.contains(" AND "));
    }

    #[test]
    fn rating_binds_owner_and_half_stars() {
        let p = membership_predicate(
            &[rule(RuleField::Rating, RuleOp::Gte, "4")],
            MatchMode::Any,
            42,
        )
        .unwrap();
        assert_eq!(p.binds, vec![Bind::Int(42), Bind::Int(8)]);
    }

    #[test]
    fn relative_window_becomes_sqlite_modifier() {
        assert_eq!(parse_relative_window("30d").unwrap(), "-30 days");
        assert_eq!(parse_relative_window("3m").unwrap(), "-3 months");
        assert_eq!(parse_relative_window("1y").unwrap(), "-1 years");
        assert!(parse_relative_window("5x").is_err());
        assert!(parse_relative_window("").is_err());
    }

    #[test]
    fn between_splits_range_and_validates_dates() {
        let p = membership_predicate(
            &[rule(
                RuleField::DateAdded,
                RuleOp::Between,
                "2025-06-01..2025-08-30",
            )],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert_eq!(
            p.binds,
            vec![
                Bind::Text("2025-06-01".into()),
                Bind::Text("2025-08-30".into())
            ]
        );
        assert!(membership_predicate(
            &[rule(RuleField::DateAdded, RuleOp::Between, "2025-06-01")],
            MatchMode::Any,
            1
        )
        .is_err());
    }

    #[test]
    fn status_finished_builds_exists_bound_to_owner() {
        let p = membership_predicate(
            &[rule(RuleField::Status, RuleOp::Is, "finished")],
            MatchMode::Any,
            42,
        )
        .unwrap();
        assert!(p.sql.contains("EXISTS"), "sql was {}", p.sql);
        assert!(
            p.sql.contains("rs.status = 'finished'"),
            "sql was {}",
            p.sql
        );
        assert_eq!(p.binds, vec![Bind::Int(42)]);
    }

    #[test]
    fn status_unread_negates_the_touched_states() {
        let p = membership_predicate(
            &[rule(RuleField::Status, RuleOp::Is, "unread")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(p.sql.contains("NOT EXISTS"), "sql was {}", p.sql);
        assert!(
            p.sql.contains("('reading', 'finished')"),
            "sql was {}",
            p.sql
        );
    }

    #[test]
    fn status_rejects_unknown_value() {
        assert!(membership_predicate(
            &[rule(RuleField::Status, RuleOp::Is, "done")],
            MatchMode::Any,
            1
        )
        .is_err());
    }

    #[test]
    fn bad_op_for_field_is_rejected() {
        // `gte` is numeric-only; author is a text field, so it must be rejected.
        assert!(membership_predicate(
            &[rule(RuleField::Author, RuleOp::Gte, "3")],
            MatchMode::Any,
            1
        )
        .is_err());
    }

    #[test]
    fn author_matches_by_name_not_id() {
        // Regression: `is` on author/series used to bind a numeric id, so a
        // typed name failed to parse. It now joins the taxonomy `name` column.
        let p = membership_predicate(
            &[rule(RuleField::Author, RuleOp::Is, "Ursula K. Le Guin")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(
            p.sql.contains("a.name = ? COLLATE NOCASE"),
            "sql was {}",
            p.sql
        );
        assert_eq!(p.binds, vec![Bind::Text("Ursula K. Le Guin".into())]);
    }

    #[test]
    fn series_is_not_negates_the_exists() {
        let p = membership_predicate(
            &[rule(RuleField::Series, RuleOp::IsNot, "Foundation")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(p.sql.starts_with("(NOT EXISTS"), "sql was {}", p.sql);
        assert!(p.sql.contains("s.name = ? COLLATE NOCASE"));
    }

    #[test]
    fn contains_and_starts_with_build_like_patterns() {
        let c = membership_predicate(
            &[rule(RuleField::Tag, RuleOp::Contains, "sci")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(
            c.sql.contains("t.name LIKE ? ESCAPE '\\'"),
            "sql was {}",
            c.sql
        );
        assert!(
            c.sql.contains("je.value LIKE ? ESCAPE '\\'"),
            "sql was {}",
            c.sql
        );
        assert_eq!(
            c.binds,
            vec![Bind::Text("%sci%".into()), Bind::Text("%sci%".into())]
        );

        let s = membership_predicate(
            &[rule(RuleField::Author, RuleOp::StartsWith, "Le")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(s.sql.contains("a.name LIKE ? ESCAPE '\\'"));
        assert_eq!(s.binds, vec![Bind::Text("Le%".into())]);
    }

    #[test]
    fn like_escape_neutralizes_wildcards() {
        // A user searching for a literal `%` or `_` must not get wildcard
        // behavior; the escaped pattern is wrapped for `contains`.
        assert_eq!(like_escape("100%_off\\x"), "100\\%\\_off\\\\x");
        let p = membership_predicate(
            &[rule(RuleField::Tag, RuleOp::Contains, "50%")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert_eq!(
            p.binds,
            vec![Bind::Text("%50\\%%".into()), Bind::Text("%50\\%%".into())]
        );
    }

    #[test]
    fn tag_condition_is_override_aware() {
        // Both membership arms must be present: canonical links gated on "no
        // subjects override", and json_each over the override array.
        let p = membership_predicate(
            &[rule(RuleField::Tag, RuleOp::StartsWith, "Sea")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(
            p.sql
                .contains("json_type(mo.overrides, '$.subjects') IS NOT NULL"),
            "sql was {}",
            p.sql
        );
        assert!(
            p.sql.contains("json_each(mo.overrides, '$.subjects')"),
            "sql was {}",
            p.sql
        );
        assert_eq!(
            p.binds,
            vec![Bind::Text("Sea%".into()), Bind::Text("Sea%".into())]
        );
    }

    #[test]
    fn genre_condition_reads_only_the_override_array() {
        // Genres have no canonical link table, so the predicate is a single
        // json_each arm over `$.genres` with one bind.
        let p = membership_predicate(
            &[rule(RuleField::Genre, RuleOp::Is, "Fantasy")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(
            p.sql.contains("json_each(mo.overrides, '$.genres')"),
            "sql was {}",
            p.sql
        );
        assert!(
            p.sql.contains("je.value = ? COLLATE NOCASE"),
            "sql was {}",
            p.sql
        );
        assert!(
            !p.sql.contains("books_tags_link"),
            "genre must not touch the tag link table; sql was {}",
            p.sql
        );
        assert_eq!(p.binds, vec![Bind::Text("Fantasy".into())]);
    }

    #[test]
    fn genre_contains_and_starts_with_build_like_patterns() {
        let c = membership_predicate(
            &[rule(RuleField::Genre, RuleOp::Contains, "50%")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(
            c.sql.contains("je.value LIKE ? ESCAPE '\\'"),
            "sql was {}",
            c.sql
        );
        assert_eq!(c.binds, vec![Bind::Text("%50\\%%".into())]);

        let s = membership_predicate(
            &[rule(RuleField::Genre, RuleOp::StartsWith, "Epic")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert_eq!(s.binds, vec![Bind::Text("Epic%".into())]);
    }

    #[test]
    fn genre_is_not_negates_the_exists() {
        let p = membership_predicate(
            &[rule(RuleField::Genre, RuleOp::IsNot, "Horror")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(p.sql.starts_with("(NOT EXISTS"), "sql was {}", p.sql);
    }

    #[test]
    fn genre_rejects_numeric_ops() {
        assert!(membership_predicate(
            &[rule(RuleField::Genre, RuleOp::Gte, "3")],
            MatchMode::Any,
            1
        )
        .is_err());
    }

    #[test]
    fn tag_is_not_negates_the_effective_match() {
        let p = membership_predicate(
            &[rule(RuleField::Tag, RuleOp::IsNot, "fiction")],
            MatchMode::Any,
            1,
        )
        .unwrap();
        assert!(p.sql.starts_with("(NOT ("), "sql was {}", p.sql);
        assert!(p.sql.contains("je.value = ? COLLATE NOCASE"));
    }
}
