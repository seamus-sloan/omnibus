//! Wire shapes for the Reading Services annotation channel: the camelCase
//! serve envelope, the tolerant PATCH parser (device input is untrusted —
//! bad entries are skipped, never 500s), the Kobo↔Omnibus color maps, and
//! the KEPUB content-id splitter.

use omnibus_db::annotations::IngestKoboAnnotation;
use omnibus_shared::{CreateHighlight, HighlightColor, UpdateHighlightNote};
use serde::Serialize;
use serde_json::Value;

/// `GET .../annotations` response envelope. `next_page_offset_token` is
/// always null — the served set is capped well below anything Kobo pages.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationsResponse {
    pub annotations: Vec<AnnotationOut>,
    pub next_page_offset_token: Option<String>,
}

/// One served annotation, in the field shape the device firmware expects.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationOut {
    pub attachments: serde_json::Map<String, Value>,
    pub client_last_modified_utc: String,
    pub context: Option<Value>,
    pub highlight_color: &'static str,
    pub highlighted_text: String,
    pub id: String,
    /// The device's own location object, round-tripped verbatim.
    pub location: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_text: Option<String>,
    pub r#type: &'static str,
}

impl AnnotationOut {
    /// Project a stored annotation into the device's wire shape, typing it as a
    /// note when a note body is present and a highlight otherwise.
    pub fn from_served(row: &omnibus_db::annotations::ServedKoboAnnotation) -> Self {
        Self {
            attachments: serde_json::Map::new(),
            client_last_modified_utc: super::super::dto::rfc3339(row.updated_at),
            context: None,
            highlight_color: color_to_kobo(row.color),
            highlighted_text: row.text.clone().unwrap_or_default(),
            id: row.client_id.clone(),
            location: serde_json::from_str(&row.kobo_location).unwrap_or(Value::Null),
            note_text: row.note.clone().filter(|n| !n.is_empty()),
            r#type: if row.note.as_deref().is_some_and(|n| !n.is_empty()) {
                "note"
            } else {
                "highlight"
            },
        }
    }
}

/// Ceiling on entries accepted per PATCH, in each direction. A real device
/// batches far less; anything beyond this is dropped and counted skipped.
const MAX_ENTRIES: usize = 500;

/// Serialized-size cap for one annotation's `location` object.
const LOCATION_MAX_BYTES: usize = 4096;

/// A PATCH body reduced to what the store needs. `skipped` counts entries
/// dropped by a cap or missing id — the handler refuses to adopt the
/// (device, book) pair when it is non-zero, so a partially-ingested backlog
/// can never be wiped by the next GET's omission.
pub struct ParsedPatch {
    pub updates: Vec<IngestKoboAnnotation>,
    pub deleted_ids: Vec<String>,
    pub skipped: usize,
}

/// Parse a device PATCH leniently. Accepts `updatedAnnotations` (and the
/// `annotations` variant), deletes from `deletedAnnotationIds` /
/// `deletedAnnotations` / `removedAnnotations` (strings or `{id}` objects),
/// and per-entry deleted flags. Later entries win on a duplicated id.
pub fn parse_patch(body: &Value) -> ParsedPatch {
    let mut parsed = ParsedPatch {
        updates: Vec::new(),
        deleted_ids: Vec::new(),
        skipped: 0,
    };
    let Some(obj) = body.as_object() else {
        parsed.skipped = 1;
        return parsed;
    };

    let mut upserts: Vec<(IngestKoboAnnotation, bool)> = Vec::new();
    for key in ["updatedAnnotations", "annotations"] {
        for entry in obj.get(key).and_then(Value::as_array).into_iter().flatten() {
            if upserts.len() >= MAX_ENTRIES {
                parsed.skipped += 1;
                continue;
            }
            match parse_upsert(entry) {
                Some(upsert) => upserts.push(upsert),
                None => parsed.skipped += 1,
            }
        }
    }

    let mut deletes: Vec<String> = Vec::new();
    for key in [
        "deletedAnnotationIds",
        "deletedAnnotations",
        "removedAnnotations",
    ] {
        for entry in obj.get(key).and_then(Value::as_array).into_iter().flatten() {
            if deletes.len() >= MAX_ENTRIES {
                parsed.skipped += 1;
                continue;
            }
            match parse_delete(entry) {
                Some(id) => deletes.push(id),
                None => parsed.skipped += 1,
            }
        }
    }

    // A per-entry deleted flag routes the id into the delete set instead.
    for (upsert, deleted) in upserts {
        if deleted {
            deletes.push(upsert.client_id);
        } else {
            parsed.updates.push(upsert);
        }
    }

    // Later entries win on a duplicated id, and a delete beats an update —
    // the device only tombstones something it no longer holds.
    parsed.updates.retain(|u| !deletes.contains(&u.client_id));
    let mut seen = std::collections::HashSet::new();
    parsed.updates.reverse();
    parsed.updates.retain(|u| seen.insert(u.client_id.clone()));
    parsed.updates.reverse();
    deletes.retain(|id| seen.insert(id.clone()));
    parsed.deleted_ids = deletes;
    parsed
}

/// One upsert entry → the store shape, or `None` when a required field is
/// missing or a cap is blown. The bool is the entry's deleted flag.
fn parse_upsert(entry: &Value) -> Option<(IngestKoboAnnotation, bool)> {
    let obj = entry.as_object()?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= CreateHighlight::CLIENT_ID_MAX_LEN)?;

    let text = obj
        .get("highlightedText")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if text.is_some_and(|t| t.chars().count() > CreateHighlight::TEXT_MAX_LEN) {
        return None;
    }
    let note = obj
        .get("noteText")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if note.is_some_and(|n| n.chars().count() > UpdateHighlightNote::NOTE_MAX_LEN) {
        return None;
    }

    let location = obj.get("location").cloned().unwrap_or(Value::Null);
    let kobo_location = serde_json::to_string(&location).ok()?;
    if kobo_location.len() > LOCATION_MAX_BYTES {
        return None;
    }

    let deleted = ["deleted", "isDeleted", "isRemoved", "_deleted"]
        .iter()
        .any(|flag| obj.get(*flag).and_then(Value::as_bool) == Some(true));

    Some((
        IngestKoboAnnotation {
            client_id: id.to_owned(),
            color: color_from_kobo(obj.get("highlightColor").and_then(Value::as_str)),
            text: text.map(str::to_owned),
            note: note.map(str::to_owned),
            kobo_location,
            // Filled in by the handler's best-effort CFI derivation.
            epub_cfi_range: None,
        },
        deleted,
    ))
}

/// One delete entry — a bare id string or an `{id}` object.
fn parse_delete(entry: &Value) -> Option<String> {
    let id = match entry {
        Value::String(s) => s.as_str(),
        Value::Object(obj) => obj.get("id").and_then(Value::as_str)?,
        _ => return None,
    };
    let id = id.trim();
    (!id.is_empty() && id.chars().count() <= CreateHighlight::CLIENT_ID_MAX_LEN)
        .then(|| id.to_owned())
}

/// Map a device color onto the closed Omnibus palette. Tries a Kobo
/// firmware hex swatch first (what [`color_to_kobo`] now emits and, per the
/// cross-referenced reference client, what a real device sends — see
/// `docs/kobo.md`), then falls back to the CSS-style names this endpoint
/// accepted before that finding (#1629) — kept so an already-synced device
/// echoing an older-shaped value, or one from an unverified firmware
/// revision, still parses. Unknown names, unrecognized hex, and a missing
/// field all land on amber — the palette default.
pub fn color_from_kobo(raw: Option<&str>) -> HighlightColor {
    let Some(raw) = raw else {
        return HighlightColor::Amber;
    };
    if let Some(color) = color_from_kobo_hex(raw) {
        return color;
    }
    let matches = |name: &str| raw.eq_ignore_ascii_case(name);
    if matches("yellow") || matches("amber") {
        HighlightColor::Amber
    } else if matches("green") {
        HighlightColor::Green
    } else if matches("blue") || matches("cyan") || matches("teal") {
        HighlightColor::Blue
    } else if matches("pink") || matches("red") || matches("magenta") || matches("rose") {
        HighlightColor::Rose
    } else if matches("purple") || matches("violet") {
        HighlightColor::Violet
    } else {
        HighlightColor::Amber
    }
}

/// The four fixed hex swatches Kobo firmware's own highlight menu renders —
/// not CSS-style names. Cross-referenced against the equivalent field in
/// bookorbit's Reading Services client (the one open-source Kobo sync
/// project this investigation found modelling the wire value as a hex
/// swatch rather than a name), since this repo has no captured device PATCH
/// to confirm it directly; see the `docs/kobo.md` note on #1629 for the
/// caveat. Firmware has no fifth swatch for violet, so it snaps to pink —
/// the nearest of the four by RGB distance, same as rose.
fn color_from_kobo_hex(raw: &str) -> Option<HighlightColor> {
    if raw.eq_ignore_ascii_case("#F6F3B3") {
        Some(HighlightColor::Amber)
    } else if raw.eq_ignore_ascii_case("#C6E09E") {
        Some(HighlightColor::Green)
    } else if raw.eq_ignore_ascii_case("#B2E1E8") {
        Some(HighlightColor::Blue)
    } else if raw.eq_ignore_ascii_case("#E8AFCF") {
        Some(HighlightColor::Rose)
    } else {
        None
    }
}

/// The reverse map, onto the hex swatch Kobo firmware renders. See
/// [`color_from_kobo_hex`] for the source and the violet caveat: it has no
/// firmware swatch of its own and renders identically to rose on-device.
pub fn color_to_kobo(color: HighlightColor) -> &'static str {
    match color {
        HighlightColor::Amber => "#F6F3B3",
        HighlightColor::Green => "#C6E09E",
        HighlightColor::Blue => "#B2E1E8",
        HighlightColor::Rose | HighlightColor::Violet => "#E8AFCF",
    }
}

/// Strip a KEPUB chapter suffix: annotation content ids can arrive
/// chapter-scoped (`<uuid>!!OEBPS/ch.xhtml`); the entitlement id is the part
/// before the separator.
pub fn parse_content_id(raw: &str) -> &str {
    raw.split("!!").next().unwrap_or(raw)
}
