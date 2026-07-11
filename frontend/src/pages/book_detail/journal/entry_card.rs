//! One reading-journal entry card: author byline, rendered markdown body, and
//! (for the entry's owner) an inline edit/delete action row backed by the
//! same progressive-enhancement editor as the composer.

use dioxus::prelude::*;
use omnibus_shared::{JournalEntry, UpdateJournalEntry, UserSummary};

use crate::data;
use crate::pages::book_detail::journal_editor::*;

/// Per-entry inline-edit signals shared by the entry card, its header action
/// row, and the edit form. Grouped so those components stay under the prop cap.
#[derive(Clone, Copy, PartialEq)]
struct JournalEntryEditState {
    editing: Signal<bool>,
    edit_body: Signal<String>,
    saving: Signal<bool>,
    error: Signal<Option<String>>,
    reload: Signal<u32>,
}

/// Presentational fields for a journal entry card header (author identity,
/// meta line, and the data an owner's Edit action needs to seed the form).
#[derive(Clone, PartialEq)]
struct JournalEntryHeaderView {
    author_name: String,
    initial: String,
    is_owner: bool,
    meta_line: String,
    entry_id: i64,
    body_for_edit: String,
    server_url: String,
}

/// Entry card header: author monogram + byline (with a "you" chip for the
/// owner) + date/progress meta, and — for the owner, while not already
/// editing — the Edit/Delete action row. Delete removes the entry and
/// reloads the feed; Edit seeds the edit-form body and opens it.
#[component]
fn BdJournalEntryHeader(view: JournalEntryHeaderView, edit: JournalEntryEditState) -> Element {
    let JournalEntryHeaderView {
        author_name,
        initial,
        is_owner,
        meta_line,
        entry_id,
        body_for_edit,
        server_url,
    } = view;
    let JournalEntryEditState {
        editing,
        edit_body,
        saving,
        error,
        reload,
    } = edit;
    let mut editing = editing;
    let mut edit_body = edit_body;
    let mut saving = saving;
    let mut error = error;
    let mut reload = reload;

    rsx! {
        div { class: "bd-journal-entry-head",
            span { class: "bd-journal-avatar", aria_hidden: "true", "{initial}" }
            div { class: "bd-journal-entry-meta",
                div { class: "bd-journal-entry-byline",
                    span { class: "bd-journal-author", "{author_name}" }
                    if is_owner {
                        span { class: "chip bd-journal-you", "you" }
                    }
                }
                div { class: "mono bd-journal-entry-date", "{meta_line}" }
            }
            if is_owner && !editing() {
                div { class: "bd-journal-entry-actions",
                    button {
                        r#type: "button",
                        class: "btn ghost sm",
                        "data-testid": "journal-edit",
                        onclick: move |_| {
                            edit_body.set(body_for_edit.clone());
                            error.set(None);
                            editing.set(true);
                        },
                        "Edit"
                    }
                    button {
                        r#type: "button",
                        class: "btn ghost sm bd-journal-delete",
                        "data-testid": "journal-delete",
                        disabled: saving(),
                        onclick: move |_| {
                            let url = server_url.clone();
                            saving.set(true);
                            error.set(None);
                            spawn(async move {
                                match data::delete_journal_entry(&url, entry_id).await {
                                    Ok(()) => reload.set(reload() + 1),
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                                saving.set(false);
                            });
                        },
                        "Delete"
                    }
                }
            }
        }
    }
}

/// One journal entry card: author monogram + byline + date/progress, rendered
/// markdown body, and owner-only inline edit / delete.
#[component]
pub(super) fn BdJournalEntryCard(
    entry: JournalEntry,
    current_user: Option<UserSummary>,
    server_url: String,
    reload: Signal<u32>,
) -> Element {
    let edit = JournalEntryEditState {
        editing: use_signal(|| false),
        edit_body: use_signal(|| entry.body_md.clone()),
        saving: use_signal(|| false),
        error: use_signal(|| None::<String>),
        reload,
    };
    let editing = edit.editing;
    let error = edit.error;

    let is_owner = current_user
        .as_ref()
        .map(|u| u.id == entry.author_id)
        .unwrap_or(false);
    let initial = entry
        .author_name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let date = fmt_long_date(entry.created_at);
    let meta_line = match entry.progress {
        Some(p) => format!("{date} \u{00b7} at {p}%"),
        None => date,
    };
    let entry_id = entry.id;
    let entry_progress = entry.progress;

    rsx! {
        article { class: "card bd-journal-entry", "data-testid": "journal-entry",
            BdJournalEntryHeader {
                view: JournalEntryHeaderView {
                    author_name: entry.author_name.clone(),
                    initial,
                    is_owner,
                    meta_line,
                    entry_id,
                    body_for_edit: entry.body_md.clone(),
                    server_url: server_url.clone(),
                },
                edit,
            }
            if editing() {
                BdJournalEntryEditForm {
                    entry_id,
                    entry_progress,
                    server_url: server_url.clone(),
                    edit,
                }
            } else {
                div {
                    class: "bd-journal-entry-body",
                    dangerous_inner_html: "{entry.body_html}",
                }
                if let Some(msg) = error() {
                    span { class: "mono bd-journal-error", role: "alert", "{msg}" }
                }
            }
        }
    }
}

/// Markdown toolbar + textarea/contenteditable pair for editing an existing
/// entry (same progressive-enhancement pattern as the composer — see
/// `BdJournalEditorBody` in [`super::composer`]), keyed per entry id so
/// multiple open editors don't collide.
#[component]
fn BdJournalEntryEditor(entry_id: i64, edit_body: Signal<String>) -> Element {
    let mut edit_body = edit_body;
    rsx! {
        div { class: "bd-journal-toolbar-row",
            BdJournalToolbar { target_id: format!("journal-edit-editor-{entry_id}") }
        }
        div { class: "bd-journal-editor-wrap",
            textarea {
                id: "journal-edit-body-{entry_id}",
                class: "me-textarea bd-journal-textarea",
                "data-testid": "journal-edit-body",
                rows: "5",
                value: "{edit_body}",
                oninput: move |e| edit_body.set(e.value()),
                onmounted: move |_| editor_enhance(
                    &format!("journal-edit-body-{entry_id}"),
                    &format!("journal-edit-editor-{entry_id}"),
                ),
            }
            div {
                id: "journal-edit-editor-{entry_id}",
                class: "me-textarea bd-journal-editor",
                "data-testid": "journal-edit-editor",
                contenteditable: "true",
                role: "textbox",
                "aria-multiline": "true",
                "aria-label": "Edit journal entry",
            }
        }
    }
}

/// Inline edit form for a journal entry: the toolbar/editor pair plus a
/// save/cancel foot. Save posts the update, then closes and reloads the feed.
#[component]
fn BdJournalEntryEditForm(
    entry_id: i64,
    entry_progress: Option<u8>,
    server_url: String,
    edit: JournalEntryEditState,
) -> Element {
    let JournalEntryEditState {
        editing,
        edit_body,
        saving,
        error,
        reload,
    } = edit;
    let mut saving = saving;
    let mut error = error;
    let mut editing = editing;
    let mut reload = reload;

    rsx! {
        BdJournalEntryEditor { entry_id, edit_body }
        div { class: "bd-journal-entry-foot",
            if let Some(msg) = error() {
                span { class: "mono bd-journal-error", role: "alert", "{msg}" }
            }
            span { class: "bd-journal-foot-spacer" }
            button {
                r#type: "button",
                class: "btn ghost sm",
                onclick: move |_| editing.set(false),
                "Cancel"
            }
            button {
                r#type: "button",
                class: "btn primary sm",
                "data-testid": "journal-edit-save",
                disabled: saving() || edit_body().trim().is_empty(),
                onclick: move |_| {
                    let url = server_url.clone();
                    let input = UpdateJournalEntry {
                        body_md: edit_body(),
                        progress: entry_progress,
                    };
                    saving.set(true);
                    error.set(None);
                    spawn(async move {
                        match data::update_journal_entry(&url, entry_id, input).await {
                            Ok(_) => {
                                editing.set(false);
                                saving.set(false);
                                reload.set(reload() + 1);
                            }
                            Err(e) => {
                                error.set(Some(e.to_string()));
                                saving.set(false);
                            }
                        }
                    });
                },
                if saving() { "Saving\u{2026}" } else { "Save" }
            }
        }
    }
}

/// Format a unix-seconds timestamp as e.g. "May 17, 2026". Dependency-free and
/// deterministic (no wall clock), so it's safe in both SSR and WASM renders.
fn fmt_long_date(unix_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let (y, m, d) = civil_from_days(unix_secs.div_euclid(86_400));
    let name = MONTHS
        .get((m as usize).saturating_sub(1))
        .copied()
        .unwrap_or("");
    format!("{name} {d}, {y}")
}

/// Convert days since the unix epoch to a `(year, month, day)` civil date
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::fmt_long_date;

    #[test]
    fn formats_known_epoch_dates() {
        // 2026-05-17 12:00:00 UTC = 1_779_019_200
        assert_eq!(fmt_long_date(1_779_019_200), "May 17, 2026");
        // The unix epoch itself.
        assert_eq!(fmt_long_date(0), "January 1, 1970");
    }
}
