//! User-supplied metadata overrides + length validation + merge semantics.
//!
//! Persisted as JSON in `metadata_overrides.overrides`. `validate` runs at
//! the handler boundary so over-long inputs return 400 instead of failing
//! deeper; `merge` lets a second edit preserve prior-override fields it
//! doesn't touch.

use serde::{Deserialize, Serialize};

use super::metadata::Contributor;

/// User-supplied metadata overrides. JSON-serialized into the
/// `metadata_overrides.overrides` column. Each field, when `Some`, replaces
/// the corresponding scanned value at read time.
///
/// M2M fields (`creators`, `subjects`, `genres`) replace entirely when
/// present — a tag list override replaces, not appends.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_index: Option<String>,
    /// ISBN-13 override. `Some("")` clears any scanned/prior-override value
    /// (mirrors the empty-string-clears convention used by the other scalar
    /// fields); a non-empty value must be exactly 13 ASCII digits, enforced
    /// by `validate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn13: Option<String>,
    /// Replaces the entire creators list when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creators: Option<Vec<Contributor>>,
    /// Replaces the entire subjects (tags) list when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subjects: Option<Vec<String>>,
    /// Replaces the entire genres list when present. Unlike every other
    /// field here this is not an *override* of a scanned value — nothing
    /// scans a genre — so this key is the sole storage for a book's genres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
}

impl MetadataOverrides {
    /// Maximum title length in Unicode scalar values (chars).
    pub const TITLE_MAX_LEN: usize = 500;
    /// Maximum length (in chars) for the `description` field.
    pub const DESCRIPTION_MAX_LEN: usize = 50_000;
    /// Maximum length in chars for single-line name fields: publisher, series, language, and creator names.
    pub const NAME_MAX_LEN: usize = 250;
    /// Maximum length (in chars) for a single tag (subject) string. Preserved
    /// from the pre-existing `MAX_SUBJECT_CHARS = 128` constant — tags are
    /// typically short controlled-vocabulary terms, so the tighter cap is
    /// intentional. `NAME_MAX_LEN = 250` applies to author/series/publisher
    /// names.
    pub const TAG_MAX_LEN: usize = 128;
    /// Maximum number of tags (subjects). `pub` because the db-layer bulk
    /// merge re-checks it after computing each book's tag delta.
    pub const MAX_SUBJECTS: usize = 64;
    /// Maximum length (in chars) for a single genre. Same budget as
    /// [`Self::TAG_MAX_LEN`] — both are short controlled-vocabulary terms.
    pub const GENRE_MAX_LEN: usize = 128;
    /// Maximum number of genres. `pub` for the same reason as
    /// [`Self::MAX_SUBJECTS`] — the bulk merge re-checks it per book.
    pub const MAX_GENRES: usize = 64;
    /// Maximum number of creators.
    pub(crate) const MAX_CREATORS: usize = 32;

    /// Validate field lengths. Returns `Err` with a human-readable message if
    /// any field exceeds its cap. Call at the handler level before persisting.
    ///
    /// Length caps are measured in Unicode scalar values (`chars`), not UTF-8
    /// bytes, so a multibyte (e.g. CJK or emoji) string is held to the same
    /// visible-character budget as an ASCII one.
    pub fn validate(&self) -> Result<(), String> {
        let check = |name: &str, val: &Option<String>, max: usize| -> Result<(), String> {
            if let Some(v) = val {
                if v.chars().count() > max {
                    return Err(format!("{name} exceeds {max} characters"));
                }
            }
            Ok(())
        };
        check("title", &self.title, Self::TITLE_MAX_LEN)?;
        check("publisher", &self.publisher, Self::NAME_MAX_LEN)?;
        check("published", &self.published, Self::NAME_MAX_LEN)?;
        check("language", &self.language, Self::NAME_MAX_LEN)?;
        check("series", &self.series, Self::NAME_MAX_LEN)?;
        check("series_index", &self.series_index, Self::NAME_MAX_LEN)?;
        check("description", &self.description, Self::DESCRIPTION_MAX_LEN)?;
        self.validate_isbn13()?;
        self.validate_creators()?;
        validate_name_list(
            self.subjects.as_deref(),
            "tag",
            Self::TAG_MAX_LEN,
            Self::MAX_SUBJECTS,
        )?;
        validate_name_list(
            self.genres.as_deref(),
            "genre",
            Self::GENRE_MAX_LEN,
            Self::MAX_GENRES,
        )?;
        Ok(())
    }

    /// ISBN-13 input rule: empty (clears the override) or exactly 13 ASCII
    /// digits. Hyphens/spaces are rejected rather than stripped — the
    /// client submits a normalized digit string. Deliberately format-only —
    /// no mod-10 check-digit verification — since this is user-editable
    /// free-text metadata, not a barcode-scanned value needing strict
    /// validation; a malformed-but-13-digit entry just renders as an
    /// incorrect ISBN, not a corrupted one.
    fn validate_isbn13(&self) -> Result<(), String> {
        let Some(ref v) = self.isbn13 else {
            return Ok(());
        };
        if v.is_empty() {
            return Ok(());
        }
        if v.chars().count() != 13 || !v.chars().all(|c| c.is_ascii_digit()) {
            return Err("ISBN-13 must be exactly 13 digits".to_string());
        }
        Ok(())
    }

    /// Per-creator length / count caps, factored out of `validate` to keep
    /// each helper short.
    fn validate_creators(&self) -> Result<(), String> {
        let Some(ref creators) = self.creators else {
            return Ok(());
        };
        if creators.len() > Self::MAX_CREATORS {
            return Err(format!("too many creators (max {})", Self::MAX_CREATORS));
        }
        for c in creators {
            if c.name.chars().count() > Self::NAME_MAX_LEN {
                return Err(format!(
                    "creator name exceeds {} characters",
                    Self::NAME_MAX_LEN
                ));
            }
            if let Some(ref role) = c.role {
                if role.chars().count() > Self::NAME_MAX_LEN {
                    return Err(format!(
                        "creator role exceeds {} characters",
                        Self::NAME_MAX_LEN
                    ));
                }
            }
            if let Some(ref file_as) = c.file_as {
                if file_as.chars().count() > Self::NAME_MAX_LEN {
                    return Err(format!(
                        "creator file_as exceeds {} characters",
                        Self::NAME_MAX_LEN
                    ));
                }
            }
        }
        Ok(())
    }

    /// Merge `incoming` on top of `self`. Fields that are `Some` in `incoming`
    /// win; `None` fields in `incoming` preserve `self`'s value. This lets a
    /// second edit that only touches two fields preserve earlier overrides on
    /// untouched fields.
    pub fn merge(&self, incoming: &MetadataOverrides) -> MetadataOverrides {
        MetadataOverrides {
            title: incoming.title.clone().or(self.title.clone()),
            description: incoming.description.clone().or(self.description.clone()),
            publisher: incoming.publisher.clone().or(self.publisher.clone()),
            published: incoming.published.clone().or(self.published.clone()),
            language: incoming.language.clone().or(self.language.clone()),
            series: incoming.series.clone().or(self.series.clone()),
            series_index: incoming.series_index.clone().or(self.series_index.clone()),
            isbn13: incoming.isbn13.clone().or(self.isbn13.clone()),
            creators: incoming.creators.clone().or(self.creators.clone()),
            subjects: incoming.subjects.clone().or(self.subjects.clone()),
            genres: incoming.genres.clone().or(self.genres.clone()),
        }
    }
}

/// Per-entry length and per-list count caps for a free-text name list
/// (tags, genres). `label` names the entry in the error message and is
/// pluralized with a bare `s` for the count message.
fn validate_name_list(
    list: Option<&[String]>,
    label: &str,
    max_len: usize,
    max_count: usize,
) -> Result<(), String> {
    let Some(list) = list else {
        return Ok(());
    };
    if list.len() > max_count {
        return Err(format!("too many {label}s (max {max_count})"));
    }
    for entry in list {
        if entry.chars().count() > max_len {
            return Err(format!("{label} exceeds {max_len} characters"));
        }
    }
    Ok(())
}

/// One edit applied to many books at once from the table view's bulk-edit
/// modal. Scalar fields: `None` = leave every book unchanged; `Some(v)` = set
/// on every book (`Some("")` stays reserved for a future explicit-clear
/// affordance). `authors` replaces each book's full creator list when `Some`
/// — an empty chip list in the UI maps to `None`, so bulk cannot clear
/// authors. Tags and genres are add/remove deltas applied per book against
/// that book's current effective list — never a wholesale replace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkMetadataEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_genres: Vec<String>,
}

impl BulkMetadataEdit {
    /// True when no field would change any book — callers reject such an
    /// edit rather than writing no-op override rows.
    pub fn is_empty(&self) -> bool {
        self.authors.is_none()
            && self.series.is_none()
            && self.publisher.is_none()
            && self.language.is_none()
            && self.add_tags.is_empty()
            && self.remove_tags.is_empty()
            && self.add_genres.is_empty()
            && self.remove_genres.is_empty()
    }

    /// Validate field lengths and counts against the same caps as
    /// [`MetadataOverrides::validate`]. Call at the handler boundary.
    ///
    /// Each delta list is validated *separately*, not concatenated: they are
    /// deltas, so their combined length is not a per-book count — a large
    /// remove plus a small add is legal even past `MAX_SUBJECTS` /
    /// `MAX_GENRES`. The true post-delta cap is enforced per book by the db
    /// merge, which is the only place that sees the resulting list.
    pub fn validate(&self) -> Result<(), String> {
        // Route through the existing validators so the caps and messages
        // cannot drift from the single-book path.
        self.scalar_overrides().validate()?;
        for list in [&self.add_tags, &self.remove_tags] {
            MetadataOverrides {
                subjects: Some(list.clone()),
                ..Default::default()
            }
            .validate()?;
        }
        for list in [&self.add_genres, &self.remove_genres] {
            MetadataOverrides {
                genres: Some(list.clone()),
                ..Default::default()
            }
            .validate()?;
        }
        Ok(())
    }

    /// The scalar/creators part as a per-book [`MetadataOverrides`]. Subjects
    /// and genres are left `None` — their deltas are applied per book by the
    /// caller via [`Self::apply_tags`] / [`Self::apply_genres`].
    pub fn scalar_overrides(&self) -> MetadataOverrides {
        MetadataOverrides {
            publisher: self.publisher.clone(),
            language: self.language.clone(),
            series: self.series.clone(),
            creators: self.authors.as_ref().map(|names| {
                names
                    .iter()
                    .map(|name| Contributor {
                        name: name.clone(),
                        ..Default::default()
                    })
                    .collect()
            }),
            ..Default::default()
        }
    }

    /// Apply the tag deltas to one book's current effective subject list.
    /// See [`apply_delta`] for the ordering rules.
    pub fn apply_tags(&self, current: &[String]) -> Vec<String> {
        apply_delta(current, &self.remove_tags, &self.add_tags)
    }

    /// Apply the genre deltas to one book's current genre list. See
    /// [`apply_delta`] for the ordering rules.
    pub fn apply_genres(&self, current: &[String]) -> Vec<String> {
        apply_delta(current, &self.remove_genres, &self.add_genres)
    }
}

/// Apply one add/remove delta to a book's current list: remove exact
/// matches first, then append `add` entries not already present, preserving
/// order (existing, then added). Remove-before-add means an entry in both
/// lists ends up present.
fn apply_delta(current: &[String], remove: &[String], add: &[String]) -> Vec<String> {
    let mut result: Vec<String> = current
        .iter()
        .filter(|v| !remove.contains(v))
        .cloned()
        .collect();
    for value in add {
        if !result.contains(value) {
            result.push(value.clone());
        }
    }
    result
}

/// Summary of a fleet-wide EPUB override bake: each book counts exactly once, into `rewritten`, `skipped`, or `errors`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkRewriteSummary {
    pub rewritten: usize,
    pub skipped: usize,
    pub errors: Vec<BulkRewriteError>,
}

/// One per-book failure from a fleet-wide EPUB rewrite pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkRewriteError {
    pub book_uuid: String,
    pub message: String,
}
