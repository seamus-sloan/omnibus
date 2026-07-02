//! Public reading-journal section for the book-detail page.
//!
//! Renders every reader's entries plus an owner-only composer with a live
//! markdown editor (see `journal_editor.js`). Entries, the current user, and
//! the write surface all attach post-mount so SSR and first-hydration paint
//! stay identical (rule 07); bodies are sanitized server-side.

use dioxus::prelude::*;
use omnibus_shared::{
    CreateJournalEntry, Highlight, JournalEntry, UpdateJournalEntry, UserSummary,
};

use crate::{data, use_server_url};

use super::journal_editor::*;
use super::BdSectionHead;

/// Public reading-journal section: header, composer, and the entry feed.
#[component]
pub(super) fn BdJournalSection(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut entries = use_signal(Vec::<JournalEntry>::new);
    let mut current_user = use_signal(|| None::<UserSummary>);
    // Bumped after any mutation to refetch the server-authoritative feed.
    let reload = use_signal(|| 0u32);

    use_effect(move || {
        spawn(async move {
            if let Ok(u) = data::current_user().await {
                current_user.set(u);
            }
        });
    });

    let load_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        let _ = reload();
        let url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Ok(list) = data::list_journal_entries(&url, &uuid).await {
                entries.set(list);
            }
        });
    }));

    // Click-to-reveal spoilers, delegated on `document` so it covers entries
    // that render after mount and bound once via a window guard. Web-only: the
    // eval is a no-op on SSR / native, and gating the body (not the hook) keeps
    // the hook count identical across targets for hydration.
    use_effect(move || {
        #[cfg(feature = "web")]
        {
            let _ = dioxus::document::eval(
                r#"
                if (!window.__omnibusSpoilerBound) {
                    window.__omnibusSpoilerBound = true;
                    document.addEventListener('click', (e) => {
                        const s = e.target.closest && e.target.closest('.spoiler');
                        if (s) s.classList.toggle('revealed');
                    });
                }
                "#,
            );
        }
    });

    let list = entries();
    let reader_count = {
        let mut ids: Vec<i64> = list.iter().map(|e| e.author_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    };
    let kicker = if list.is_empty() {
        "Reading journal · no entries yet".to_string()
    } else {
        let entry_word = if list.len() == 1 { "entry" } else { "entries" };
        let reader_word = if reader_count == 1 {
            "reader"
        } else {
            "readers"
        };
        format!(
            "Reading journal · {} {entry_word} from {reader_count} {reader_word}",
            list.len()
        )
    };

    rsx! {
        div { id: "journal", class: "bd-journal", "data-testid": "journal-section",
            BdSectionHead { kicker, title: "What readers have written".to_string() }
            p { class: "mono bd-journal-blurb",
                "A shared log — every reader's journal for this book lives here."
            }
            BdJournalComposer { uuid: uuid.clone(), server_url: server_url.clone(), reload }
            if list.is_empty() {
                div { class: "bd-journal-empty card", "data-testid": "journal-empty",
                    p { class: "mono", "No journal entries yet — be the first to write one." }
                }
            } else {
                div { class: "bd-journal-list", "data-testid": "journal-list",
                    for entry in list.iter() {
                        BdJournalEntryCard {
                            key: "{entry.id}",
                            entry: entry.clone(),
                            current_user: current_user(),
                            server_url: server_url.clone(),
                            reload,
                        }
                    }
                }
            }
        }
    }
}

/// Collapsed prompt → expanded markdown composer with a Write/Preview toggle, an
/// optional reading-progress slider, and a spoiler-syntax hint.
#[component]
fn BdJournalComposer(uuid: String, server_url: String, reload: Signal<u32>) -> Element {
    let mut reload = reload;
    let mut open = use_signal(|| false);
    let mut body = use_signal(String::new);
    let mut show_preview = use_signal(|| false);
    let mut preview_html = use_signal(String::new);
    let mut track_progress = use_signal(|| false);
    let mut progress = use_signal(|| 50i64);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    // Saved highlights for this book, loaded lazily the first time the
    // insert-from-highlights popover is opened.
    let mut highlights = use_signal(Vec::<Highlight>::new);
    let mut highlights_open = use_signal(|| false);
    let mut highlights_loaded = use_signal(|| false);

    if !open() {
        return rsx! {
            button {
                r#type: "button",
                class: "card bd-journal-prompt",
                "data-testid": "journal-open-composer",
                onclick: move |_| open.set(true),
                span { class: "bd-journal-prompt-text", "Write a journal entry on this book\u{2026}" }
                span { class: "bd-journal-prompt-plus", aria_hidden: "true", "+" }
            }
        };
    }

    rsx! {
        div { class: "card bd-journal-composer", "data-testid": "journal-composer",
            div { class: "bd-journal-composer-tabs",
                button {
                    r#type: "button",
                    class: if show_preview() { "btn ghost sm" } else { "btn ghost sm bd-tab-active" },
                    onclick: move |_| show_preview.set(false),
                    "Write"
                }
                button {
                    r#type: "button",
                    class: if show_preview() { "btn ghost sm bd-tab-active" } else { "btn ghost sm" },
                    "data-testid": "journal-preview-toggle",
                    onclick: {
                        let url = server_url.clone();
                        move |_| {
                            let url = url.clone();
                            let md = body();
                            // Clear any prior preview so a failed fetch can't leave stale HTML on screen.
                            preview_html.set(String::new());
                            show_preview.set(true);
                            spawn(async move {
                                match data::preview_journal_markdown(&url, md).await {
                                    Ok(html) => {
                                        preview_html.set(html);
                                        error.set(None);
                                    }
                                    Err(e) => error.set(Some(format!("Preview failed: {e}"))),
                                }
                            });
                        }
                    },
                    "Preview"
                }
                span { class: "bd-journal-foot-spacer" }
                span {
                    class: "mono bd-journal-spoiler-help",
                    "data-testid": "journal-spoiler-help",
                    title: "Hide spoilers with ||double pipes|| \u{2014} they appear blurred until clicked.",
                    "Spoilers? Wrap text in ||double pipes||"
                }
            }
            if !show_preview() {
                div { class: "bd-journal-toolbar-row",
                    BdJournalToolbar { target_id: "journal-composer-editor".to_string() }
                    div { class: "bd-journal-hl-wrap",
                        button {
                            r#type: "button",
                            class: if highlights_open() { "btn ghost sm bd-tab-active" } else { "btn ghost sm" },
                            "data-testid": "journal-insert-highlight",
                            onclick: {
                                let url = server_url.clone();
                                let uuid = uuid.clone();
                                move |_| {
                                    let opening = !highlights_open();
                                    highlights_open.set(opening);
                                    if opening && !highlights_loaded() {
                                        highlights_loaded.set(true);
                                        let url = url.clone();
                                        let uuid = uuid.clone();
                                        spawn(async move {
                                            if let Ok(list) = data::list_highlights(&url, &uuid).await {
                                                highlights.set(
                                                    list.into_iter().filter(|h| h.text.is_some()).collect(),
                                                );
                                            }
                                        });
                                    }
                                }
                            },
                            "From highlights"
                        }
                        if highlights_open() {
                            div {
                                class: "card bd-journal-hl-pop",
                                "data-testid": "journal-highlights-pop",
                                div { class: "label bd-journal-hl-head", "From your highlights" }
                                if highlights().is_empty() {
                                    p { class: "mono bd-journal-hl-empty",
                                        "No saved highlights for this book yet."
                                    }
                                } else {
                                    for h in highlights().iter() {
                                        button {
                                            r#type: "button",
                                            class: "bd-journal-hl-item",
                                            "data-testid": "journal-highlight-item",
                                            onmousedown: move |e| e.prevent_default(),
                                            onclick: {
                                                let text = h.text.clone().unwrap_or_default();
                                                move |_| {
                                                    editor_insert(
                                                        "journal-composer-editor",
                                                        &highlight_blockquote(&text),
                                                    );
                                                    highlights_open.set(false);
                                                }
                                            },
                                            "{h.text.clone().unwrap_or_default()}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if show_preview() {
                div {
                    class: "bd-journal-preview",
                    "data-testid": "journal-preview",
                    dangerous_inner_html: "{preview_html}",
                }
            } else {
                // Rule 07 (hydration parity): render one rsx body on every
                // target. SSR + first-hydration paint show the plain textarea
                // as the write surface; a sibling contenteditable div rides
                // along, hidden by CSS until post-mount JS flips the wrapper's
                // `data-omnibus-enhanced` marker (see `editor_enhance`). The
                // textarea keeps carrying the `body` signal even after
                // enhancement — the JS mirrors the editor's markdown back into
                // its `value`, so publish/validate/preview are unchanged.
                div { class: "bd-journal-editor-wrap",
                    textarea {
                        id: "journal-composer-body",
                        class: "me-textarea bd-journal-textarea",
                        "data-testid": "journal-body",
                        rows: "5",
                        placeholder: "What are you thinking about this book? Markdown supported.",
                        value: "{body}",
                        oninput: move |e| body.set(e.value()),
                        onmounted: move |_| editor_enhance(
                            "journal-composer-body",
                            "journal-composer-editor",
                        ),
                    }
                    div {
                        id: "journal-composer-editor",
                        class: "me-textarea bd-journal-editor",
                        "data-testid": "journal-editor",
                        contenteditable: "true",
                        role: "textbox",
                        "aria-multiline": "true",
                        "aria-label": "Journal entry",
                        "data-placeholder": "What are you thinking about this book? Markdown supported.",
                    }
                }
            }
            div { class: "bd-journal-composer-foot",
                label { class: "bd-journal-progress",
                    input {
                        r#type: "checkbox",
                        checked: track_progress(),
                        oninput: move |e| track_progress.set(e.value() == "true"),
                    }
                    span { class: "label", "Progress" }
                    if track_progress() {
                        input {
                            r#type: "range",
                            min: "0",
                            max: "100",
                            value: "{progress}",
                            "data-testid": "journal-progress",
                            oninput: move |e| {
                                if let Ok(v) = e.value().parse::<i64>() {
                                    progress.set(v);
                                }
                            },
                        }
                        span { class: "mono bd-journal-progress-val", "{progress}%" }
                    }
                }
                span { class: "bd-journal-foot-spacer" }
                if let Some(msg) = error() {
                    span { class: "mono bd-journal-error", role: "alert", "{msg}" }
                }
                button {
                    r#type: "button",
                    class: "btn ghost sm",
                    onclick: move |_| {
                        open.set(false);
                        body.set(String::new());
                        show_preview.set(false);
                        error.set(None);
                    },
                    "Cancel"
                }
                button {
                    r#type: "button",
                    class: "btn primary sm",
                    "data-testid": "journal-publish",
                    disabled: saving() || body().trim().is_empty(),
                    onclick: {
                        let url = server_url.clone();
                        let uuid = uuid.clone();
                        move |_| {
                            let url = url.clone();
                            let input = CreateJournalEntry {
                                book_uuid: uuid.clone(),
                                body_md: body(),
                                progress: if track_progress() { Some(progress() as u8) } else { None },
                            };
                            saving.set(true);
                            error.set(None);
                            spawn(async move {
                                match data::create_journal_entry(&url, input).await {
                                    Ok(_) => {
                                        body.set(String::new());
                                        show_preview.set(false);
                                        track_progress.set(false);
                                        open.set(false);
                                        reload.set(reload() + 1);
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                                saving.set(false);
                            });
                        }
                    },
                    if saving() { "Publishing\u{2026}" } else { "Publish entry" }
                }
            }
        }
    }
}

/// One journal entry card: author monogram + byline + date/progress, rendered
/// markdown body, and owner-only inline edit / delete.
#[component]
fn BdJournalEntryCard(
    entry: JournalEntry,
    current_user: Option<UserSummary>,
    server_url: String,
    reload: Signal<u32>,
) -> Element {
    let mut reload = reload;
    let mut editing = use_signal(|| false);
    let mut edit_body = use_signal(|| entry.body_md.clone());
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

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
    let body_for_edit = entry.body_md.clone();
    let entry_id = entry.id;
    let entry_progress = entry.progress;

    rsx! {
        article { class: "card bd-journal-entry", "data-testid": "journal-entry",
            div { class: "bd-journal-entry-head",
                span { class: "bd-journal-avatar", aria_hidden: "true", "{initial}" }
                div { class: "bd-journal-entry-meta",
                    div { class: "bd-journal-entry-byline",
                        span { class: "bd-journal-author", "{entry.author_name}" }
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
                            onclick: {
                                let url = server_url.clone();
                                move |_| {
                                    let url = url.clone();
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match data::delete_journal_entry(&url, entry_id).await {
                                            Ok(()) => reload.set(reload() + 1),
                                            Err(e) => {
                                                error.set(Some(e.to_string()));
                                                saving.set(false);
                                            }
                                        }
                                    });
                                }
                            },
                            "Delete"
                        }
                    }
                }
            }
            if editing() {
                div { class: "bd-journal-toolbar-row",
                    BdJournalToolbar { target_id: format!("journal-edit-editor-{entry_id}") }
                }
                // Rule 07 (hydration parity): single rsx body on every target,
                // keyed per entry id so multiple open editors don't collide.
                // Same progressive-enhancement pattern as the composer.
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
                        onclick: {
                            let url = server_url.clone();
                            move |_| {
                                let url = url.clone();
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
                            }
                        },
                        if saving() { "Saving\u{2026}" } else { "Save" }
                    }
                }
            } else {
                div {
                    class: "bd-journal-entry-body",
                    dangerous_inner_html: "{entry.body_html}",
                }
                if !is_owner {
                    if let Some(msg) = error() {
                        span { class: "mono bd-journal-error", role: "alert", "{msg}" }
                    }
                }
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
