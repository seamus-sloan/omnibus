//! Smart-rule → SQL predicate translation.
//!
//! Each [`ShelfRule`] becomes an `EXISTS`/comparison fragment over the `books b`
//! alias; the rule set combines with `OR` (match any) or `AND` (match all).
//! Owner-scoped fields (`rating`) bind the shelf owner's id. `status` is not
//! supported yet (the schema records no read-completion signal).

use omnibus_shared::{MatchMode, RuleField, RuleOp, ShelfRule};

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
    let v = rule.value.trim();
    match rule.field {
        RuleField::Tag => {
            let exists = "SELECT 1 FROM books_tags_link btl JOIN tags t ON t.id = btl.tag \
                          WHERE btl.book = b.id AND t.name = ? COLLATE NOCASE";
            let sql = match rule.op {
                RuleOp::Is => format!("EXISTS ({exists})"),
                RuleOp::IsNot => format!("NOT EXISTS ({exists})"),
                _ => return Err(unsupported(rule)),
            };
            Ok((sql, vec![Bind::Text(v.to_string())]))
        }
        RuleField::Author => {
            expect_op(rule, RuleOp::Is)?;
            Ok((
                "EXISTS (SELECT 1 FROM books_authors_link bal \
                 WHERE bal.book = b.id AND bal.author = ?)"
                    .into(),
                vec![Bind::Int(parse_id(v)?)],
            ))
        }
        RuleField::Series => {
            expect_op(rule, RuleOp::Is)?;
            Ok((
                "EXISTS (SELECT 1 FROM books_series_link bsl \
                 WHERE bsl.book = b.id AND bsl.series = ?)"
                    .into(),
                vec![Bind::Int(parse_id(v)?)],
            ))
        }
        RuleField::Format => {
            expect_op(rule, RuleOp::Includes)?;
            // `book_files.format` is COLLATE NOCASE, so a lowercase chip
            // (`"epub"`) matches the stored `"EPUB"`.
            Ok((
                "EXISTS (SELECT 1 FROM book_files bf \
                 WHERE bf.book_id = b.id AND bf.format = ? COLLATE NOCASE)"
                    .into(),
                vec![Bind::Text(v.to_string())],
            ))
        }
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
        RuleField::Status => Err(ShelfError::InvalidRule(
            "status rules are not supported yet".into(),
        )),
    }
}

/// Date-field conditions over `books.timestamp` / `books.last_modified`.
fn date_condition(rule: &ShelfRule, v: &str) -> Result<(String, Vec<Bind>), ShelfError> {
    let col = match rule.field {
        RuleField::DateAdded => "b.timestamp",
        RuleField::DateUpdated => "b.last_modified",
        _ => unreachable!("date_condition called for non-date field"),
    };
    match rule.op {
        RuleOp::InLast => Ok((
            format!("{col} >= datetime('now', ?)"),
            vec![Bind::Text(parse_relative_window(v)?)],
        )),
        RuleOp::Between => {
            let (start, end) = v.split_once("..").ok_or_else(|| {
                ShelfError::InvalidRule(format!("range must be START..END, got {v:?}"))
            })?;
            Ok((
                format!("date({col}) BETWEEN ? AND ?"),
                vec![
                    Bind::Text(validate_date(start)?),
                    Bind::Text(validate_date(end)?),
                ],
            ))
        }
        RuleOp::Before => Ok((
            format!("date({col}) < ?"),
            vec![Bind::Text(validate_date(v)?)],
        )),
        RuleOp::After => Ok((
            format!("date({col}) > ?"),
            vec![Bind::Text(validate_date(v)?)],
        )),
        _ => Err(unsupported(rule)),
    }
}

/// Error unless `rule.op` equals `want`.
fn expect_op(rule: &ShelfRule, want: RuleOp) -> Result<(), ShelfError> {
    if rule.op == want {
        Ok(())
    } else {
        Err(unsupported(rule))
    }
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
    Ok((stars * 2.0).round() as i64)
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
        assert_eq!(p.binds.len(), 2);
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
    fn status_field_is_rejected() {
        assert!(membership_predicate(
            &[rule(RuleField::Status, RuleOp::Is, "finished")],
            MatchMode::Any,
            1
        )
        .is_err());
    }

    #[test]
    fn bad_op_for_field_is_rejected() {
        assert!(membership_predicate(
            &[rule(RuleField::Author, RuleOp::IsNot, "3")],
            MatchMode::Any,
            1
        )
        .is_err());
    }
}
