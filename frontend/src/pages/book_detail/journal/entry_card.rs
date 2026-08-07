//! One reading-journal entry card: author byline, rendered markdown body, and
//! (for the entry's owner) an inline edit/delete action row backed by the
//! same progressive-enhancement editor as the composer.

use dioxus::prelude::*;
use omnibus_shared::{JournalEntry, JournalStatus, UpdateJournalEntry, UserSummary};

use crate::components::user_avatar::UserAvatar;
use crate::data;
use crate::pages::book_detail::dates::fmt_long_date;
use crate::pages::book_detail::journal_editor::*;

const JOURNAL_COLLAPSE_CHAR_THRESHOLD: usize = 900;
const JOURNAL_COLLAPSE_LINE_THRESHOLD: usize = 12;

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
    author_id: i64,
    author_has_avatar: bool,
    is_owner: bool,
    is_draft: bool,
    meta_line: String,
    entry_id: i64,
    body_for_edit: String,
    entry_progress: Option<u8>,
    server_url: String,
}

/// "Publish" button for an owner's draft entry — flips the entry's status to
/// `Published`. Split out of [`BdJournalEntryHeader`] to keep it under the
/// line cap.
#[component]
fn BdJournalPublishDraftButton(
    server_url: String,
    entry_id: i64,
    body_for_edit: String,
    entry_progress: Option<u8>,
    edit: JournalEntryEditState,
) -> Element {
    let JournalEntryEditState {
        mut saving,
        mut error,
        mut reload,
        ..
    } = edit;
    rsx! {
        button {
            r#type: "button",
            class: "btn primary sm",
            "data-testid": "journal-publish-draft",
            disabled: saving(),
            onclick: move |_| {
                let url = server_url.clone();
                let input = UpdateJournalEntry {
                    body_md: body_for_edit.clone(),
                    progress: entry_progress,
                    status: Some(JournalStatus::Published),
                };
                saving.set(true);
                error.set(None);
                spawn(async move {
                    match data::update_journal_entry(&url, entry_id, input).await {
                        Ok(_) => reload.set(reload() + 1),
                        Err(e) => error.set(Some(e.to_string())),
                    }
                    saving.set(false);
                });
            },
            "Publish"
        }
    }
}

/// Entry card header: author monogram + byline (with a "you" chip for the
/// owner) + date/progress meta, and — for the owner, while not already
/// editing — the Edit/Delete action row. Delete removes the entry and
/// reloads the feed; Edit seeds the edit-form body and opens it.
#[component]
fn BdJournalEntryHeader(view: JournalEntryHeaderView, edit: JournalEntryEditState) -> Element {
    let JournalEntryHeaderView {
        author_name,
        author_id,
        author_has_avatar,
        is_owner,
        is_draft,
        meta_line,
        entry_id,
        body_for_edit,
        entry_progress,
        server_url,
    } = view;
    let editing = edit.editing;

    rsx! {
        div { class: "bd-journal-entry-head",
            UserAvatar {
                user_id: author_id,
                name: author_name.clone(),
                has_avatar: author_has_avatar,
                class: "bd-journal-avatar",
            }
            BdJournalEntryByline { author_name, is_owner, is_draft, meta_line }
            if is_owner && !editing() {
                BdJournalEntryActions {
                    server_url,
                    entry_id,
                    body_for_edit,
                    entry_progress,
                    is_draft,
                    edit,
                }
            }
        }
    }
}

/// Author name, "you" / "Draft" chips, and the date/progress meta line.
#[component]
fn BdJournalEntryByline(
    author_name: String,
    is_owner: bool,
    is_draft: bool,
    meta_line: String,
) -> Element {
    rsx! {
        div { class: "bd-journal-entry-meta",
            div { class: "bd-journal-entry-byline",
                span { class: "bd-journal-author", "{author_name}" }
                if is_owner {
                    span { class: "chip bd-journal-you", "you" }
                }
                if is_draft {
                    span {
                        class: "chip bd-journal-draft",
                        "data-testid": "journal-draft-chip",
                        "Draft"
                    }
                }
            }
            div { class: "mono bd-journal-entry-date", "{meta_line}" }
        }
    }
}

/// Owner-only action row: optional Publish (drafts only), Edit, and Delete.
#[component]
fn BdJournalEntryActions(
    server_url: String,
    entry_id: i64,
    body_for_edit: String,
    entry_progress: Option<u8>,
    is_draft: bool,
    edit: JournalEntryEditState,
) -> Element {
    let JournalEntryEditState {
        mut editing,
        mut edit_body,
        mut saving,
        mut error,
        mut reload,
    } = edit;

    rsx! {
        div { class: "bd-journal-entry-actions",
            if is_draft {
                BdJournalPublishDraftButton {
                    server_url: server_url.clone(),
                    entry_id,
                    body_for_edit: body_for_edit.clone(),
                    entry_progress,
                    edit,
                }
            }
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
    let mut expanded = use_signal(|| false);
    let editing = edit.editing;
    let error = edit.error;

    let is_owner = current_user
        .as_ref()
        .map(|u| u.id == entry.author_id)
        .unwrap_or(false);
    let date = fmt_long_date(entry.created_at);
    let meta_line = match entry.progress {
        Some(p) => format!("{date} \u{00b7} at {p}%"),
        None => date,
    };
    let entry_id = entry.id;
    let entry_progress = entry.progress;
    let can_expand = should_show_more_button(&entry.body_md);
    let body_class = if can_expand && !expanded() {
        "bd-journal-entry-body bd-journal-entry-body-collapsed"
    } else {
        "bd-journal-entry-body"
    };

    rsx! {
        article { class: "card bd-journal-entry", "data-testid": "journal-entry",
            BdJournalEntryHeader {
                view: JournalEntryHeaderView {
                    author_name: entry.author_name.clone(),
                    author_id: entry.author_id,
                    author_has_avatar: entry.author_has_avatar,
                    is_owner,
                    is_draft: entry.status == JournalStatus::Draft,
                    meta_line,
                    entry_id,
                    body_for_edit: entry.body_md.clone(),
                    entry_progress,
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
                    class: "{body_class}",
                    dangerous_inner_html: "{entry.body_html}",
                }
                if can_expand && !expanded() {
                    button {
                        r#type: "button",
                        class: "btn ghost sm bd-journal-show-more",
                        "data-testid": "journal-show-more",
                        "aria-expanded": "{expanded()}",
                        onclick: move |_| expanded.set(true),
                        "Show more \u{2193}"
                    }
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
fn BdJournalEntryEditor(
    entry_id: i64,
    edit_body: Signal<String>,
    server_url: String,
    error: Signal<Option<String>>,
) -> Element {
    let mut edit_body = edit_body;
    rsx! {
        div { class: "bd-journal-toolbar-row",
            BdJournalToolbar {
                target_id: format!("journal-edit-editor-{entry_id}"),
                server_url,
                error,
            }
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
        BdJournalEntryEditor {
            entry_id,
            edit_body,
            server_url: server_url.clone(),
            error,
        }
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
                        // Editing never flips draft/published — the card's
                        // Publish action owns that transition.
                        status: None,
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

fn should_show_more_button(markdown: &str) -> bool {
    if markdown
        .trim()
        .chars()
        .take(JOURNAL_COLLAPSE_CHAR_THRESHOLD + 1)
        .count()
        > JOURNAL_COLLAPSE_CHAR_THRESHOLD
    {
        return true;
    }

    markdown
        .lines()
        .take(JOURNAL_COLLAPSE_LINE_THRESHOLD + 1)
        .count()
        > JOURNAL_COLLAPSE_LINE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::should_show_more_button;

    #[test]
    fn shows_more_for_long_markdown() {
        let long = "x".repeat(901);
        assert!(should_show_more_button(&long));
        let multiline = (0..13).map(|_| "line").collect::<Vec<_>>().join("\n");
        assert!(should_show_more_button(&multiline));
    }

    #[test]
    fn skips_show_more_for_short_markdown() {
        assert!(!should_show_more_button("Short entry"));
        let exactly_twelve = (0..12).map(|_| "line").collect::<Vec<_>>().join("\n");
        assert!(!should_show_more_button(&exactly_twelve));
    }
}
