//! Shelf wire types shared between web (`#[server]`) and mobile (`reqwest`).
//!
//! A shelf is a named library subset: **smart** (membership from a `field op
//! value` rule over normalized metadata) or **manual** (an explicit, ordered
//! book list). The condition shape (`RuleField`/`RuleOp`/`ShelfRule`) is defined
//! once here so F1.5 advanced search can reuse it.

use serde::{Deserialize, Serialize};

use crate::EbookMetadata;

/// Max length of a shelf name (matches the create-modal input expectation).
pub const SHELF_NAME_MAX_LEN: usize = 120;

/// Kind of shelf, fixed at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShelfKind {
    Smart,
    Manual,
}

/// Who can see a shelf. `Private` = owner + admins; `Public` = every user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

/// How smart conditions combine: `Any` = OR, `All` = AND.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    Any,
    All,
}

/// A field a smart rule can match on. Mirrors the F1.5 query vocabulary plus
/// the acquisition-date fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleField {
    Tag,
    Author,
    Series,
    Rating,
    Status,
    Format,
    Year,
    /// `books.timestamp` — when the book was first indexed.
    DateAdded,
    /// `books.last_modified` — when the book's file last changed.
    DateUpdated,
}

/// A comparison operator for a smart rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOp {
    Is,
    IsNot,
    /// case-insensitive substring match — text fields.
    Contains,
    /// case-insensitive prefix match — text fields.
    StartsWith,
    /// "is at least" (≥) — rating / year.
    Gte,
    /// membership test — format.
    Includes,
    /// relative window: value `30d` | `2w` | `3m` | `1y`.
    InLast,
    /// absolute range: value `START..END` (ISO dates).
    Between,
    Before,
    After,
}

macro_rules! wire_enum {
    ($ty:ty, $($variant:path => $token:literal),+ $(,)?) => {
        impl $ty {
            /// Stable DB/wire token.
            pub fn as_str(self) -> &'static str {
                match self { $($variant => $token),+ }
            }
            /// Parse from a DB/wire token; `None` if unrecognized. Named
            /// `from_str` for symmetry with `as_str` — it is not the fallible
            /// `FromStr` trait (no `Err` payload), hence the allow.
            #[allow(clippy::should_implement_trait)]
            pub fn from_str(s: &str) -> Option<Self> {
                Some(match s { $($token => $variant),+, _ => return None })
            }
        }
    };
}

wire_enum!(ShelfKind, ShelfKind::Smart => "smart", ShelfKind::Manual => "manual");
wire_enum!(Visibility, Visibility::Private => "private", Visibility::Public => "public");
wire_enum!(MatchMode, MatchMode::Any => "any", MatchMode::All => "all");
wire_enum!(
    RuleField,
    RuleField::Tag => "tag",
    RuleField::Author => "author",
    RuleField::Series => "series",
    RuleField::Rating => "rating",
    RuleField::Status => "status",
    RuleField::Format => "format",
    RuleField::Year => "year",
    RuleField::DateAdded => "date_added",
    RuleField::DateUpdated => "date_updated",
);
wire_enum!(
    RuleOp,
    RuleOp::Is => "is",
    RuleOp::IsNot => "is_not",
    RuleOp::Contains => "contains",
    RuleOp::StartsWith => "starts_with",
    RuleOp::Gte => "gte",
    RuleOp::Includes => "includes",
    RuleOp::InLast => "in_last",
    RuleOp::Between => "between",
    RuleOp::Before => "before",
    RuleOp::After => "after",
);

impl RuleField {
    /// `true` if `op` is valid for this field.
    pub fn accepts(self, op: RuleOp) -> bool {
        use RuleField::*;
        use RuleOp::*;
        let ok: &[RuleOp] = match self {
            Tag | Author | Series => &[Is, IsNot, Contains, StartsWith],
            Rating => &[Is, Gte],
            Status => &[Is],
            Format => &[Includes, Contains, StartsWith],
            Year => &[Is, Gte],
            DateAdded | DateUpdated => &[InLast, Between, Before, After],
        };
        ok.contains(&op)
    }

    /// `true` for the two acquisition-date fields (owner-agnostic, library-global).
    pub fn is_date(self) -> bool {
        matches!(self, RuleField::DateAdded | RuleField::DateUpdated)
    }

    /// `true` for fields resolved against the shelf owner's per-user data.
    pub fn is_per_user(self) -> bool {
        matches!(self, RuleField::Rating | RuleField::Status)
    }
}

/// One smart-shelf condition. `value` interpretation is per-field:
/// tag/author/series/status/format → name (case-insensitive); year/rating → int;
/// date fields → ISO date, `START..END`, or `30d`/`3m`/`1y`. (Migration `0034`'s
/// comment predates the author/series id→name switch and is frozen once applied;
/// this doc is the source of truth.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShelfRule {
    pub field: RuleField,
    pub op: RuleOp,
    pub value: String,
}

impl ShelfRule {
    /// Reject an op the field doesn't accept or an empty value.
    pub fn validate(&self) -> Result<(), String> {
        if !self.field.accepts(self.op) {
            return Err(format!(
                "operator {} is not valid for field {}",
                self.op.as_str(),
                self.field.as_str()
            ));
        }
        if self.value.trim().is_empty() {
            return Err("condition value is required".into());
        }
        Ok(())
    }
}

/// Compact shelf row for the rail: identity, kind, visibility, accent, count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShelfSummary {
    pub id: i64,
    pub owner_user_id: i64,
    pub kind: ShelfKind,
    pub name: String,
    pub visibility: Visibility,
    pub accent: Option<String>,
    pub book_count: i64,
}

/// Full shelf detail: the summary fields plus description, rule, and match mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shelf {
    pub id: i64,
    pub owner_user_id: i64,
    pub kind: ShelfKind,
    pub name: String,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub accent: Option<String>,
    /// `Some` for smart shelves, `None` for manual.
    pub match_mode: Option<MatchMode>,
    pub rules: Vec<ShelfRule>,
    pub book_count: i64,
}

/// Create payload. For smart: `match_mode` + `rules` set, `book_uuids` empty.
/// For manual: `match_mode`/`rules` empty, `book_uuids` optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateShelfRequest {
    pub kind: ShelfKind,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub match_mode: Option<MatchMode>,
    #[serde(default)]
    pub rules: Vec<ShelfRule>,
    #[serde(default)]
    pub book_uuids: Vec<String>,
}

impl CreateShelfRequest {
    /// Validate name + kind-specific invariants. Handlers translate `Err(_)`
    /// into a 400/422.
    pub fn validate(&self) -> Result<(), String> {
        validate_name(&self.name)?;
        match self.kind {
            ShelfKind::Smart => {
                if self.match_mode.is_none() {
                    return Err("smart shelves need a match mode".into());
                }
                if self.rules.is_empty() {
                    return Err("smart shelves need at least one condition".into());
                }
                if !self.book_uuids.is_empty() {
                    return Err("smart shelves cannot have hand-picked books".into());
                }
                for r in &self.rules {
                    r.validate()?;
                }
            }
            ShelfKind::Manual => {
                if !self.rules.is_empty() || self.match_mode.is_some() {
                    return Err("manual shelves cannot have rules".into());
                }
            }
        }
        Ok(())
    }
}

/// Partial update. `None` fields are left unchanged. `rules` replaces the whole
/// rule set (smart shelves only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateShelfRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
    #[serde(default)]
    pub match_mode: Option<MatchMode>,
    #[serde(default)]
    pub rules: Option<Vec<ShelfRule>>,
}

impl UpdateShelfRequest {
    /// Validate any present fields.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(name) = &self.name {
            validate_name(name)?;
        }
        if let Some(rules) = &self.rules {
            for r in rules {
                r.validate()?;
            }
        }
        Ok(())
    }
}

/// Live-preview result for the create modal: "N of M match" + sample covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulePreview {
    pub matched: i64,
    pub total: i64,
    pub sample: Vec<EbookMetadata>,
}

/// Unsaved rule sent to the preview endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulePreviewRequest {
    pub match_mode: MatchMode,
    pub rules: Vec<ShelfRule>,
}

/// One page of a shelf's books (v1: capped, no cursor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShelfPage {
    pub books: Vec<EbookMetadata>,
}

/// Reject an empty or over-long shelf name.
fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("shelf name is required".into());
    }
    if trimmed.chars().count() > SHELF_NAME_MAX_LEN {
        return Err(format!(
            "shelf name must be ≤ {SHELF_NAME_MAX_LEN} characters"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
