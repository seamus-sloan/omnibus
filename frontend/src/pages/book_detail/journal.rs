//! Public reading-journal section for the book-detail page. Renders every
//! reader's entries plus an owner-only composer with a live markdown editor
//! (see `journal_editor.js`). Entries, the current user, and the write surface
//! all attach post-mount so SSR and first-hydration paint stay identical
//! (rule 07); bodies are sanitized server-side.

use dioxus::prelude::*;
use omnibus_shared::{
    CreateJournalEntry, Highlight, JournalEntry, UpdateJournalEntry, UserSummary,
};

use crate::{data, use_server_url};

use super::journal_editor::*;
use super::BdSectionHead;

/// Draft-composer signals owned by [`BdJournalComposer`] and threaded into
/// its footer. Grouped so the composer sub-components stay under the prop cap.
#[derive(Clone, Copy, PartialEq)]
struct JournalComposerState {
    open: Signal<bool>,
    body: Signal<String>,
    track_progress: Signal<bool>,
    progress: Signal<i64>,
    saving: Signal<bool>,
    error: Signal<Option<String>>,
    show_preview: Signal<bool>,
}

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

/// Public reading-journal section: header, composer, and the entry feed.
#[component]
pub(super) fn BdJournalSection(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut entries = use_signal(Vec::<JournalEntry>::new);
    // Derived from the app-wide `CurrentUser` context (`crate::use_current_user_summary`)
    // instead of an independent per-mount `/api/auth/me` fetch. Mobile/SSR
    // stay at the `None` default since the context is web-only.
    let current_user = crate::use_current_user_summary();
    // Bumped after any mutation to refetch the server-authoritative feed.
    let reload = use_signal(|| 0u32);

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
    // that render after mount and bound once via a window guard. The sanitizer
    // emits the spoiler as a real `<button>`, so Tab reaches it and Enter/Space
    // fire a native click without a keydown listener; the handler just needs
    // to keep `aria-expanded` in sync with the `.revealed` class so assistive
    // tech reflects the toggled state. Web-only: the eval is a no-op on SSR /
    // native, and gating the body (not the hook) keeps the hook count
    // identical across targets for hydration.
    use_effect(move || {
        #[cfg(feature = "web")]
        {
            let _ = dioxus::document::eval(
                r#"
                if (!window.__omnibusSpoilerBound) {
                    window.__omnibusSpoilerBound = true;
                    document.addEventListener('click', (e) => {
                        const s = e.target.closest && e.target.closest('.spoiler');
                        if (!s) return;
                        const revealed = s.classList.toggle('revealed');
                        s.setAttribute('aria-expanded', revealed ? 'true' : 'false');
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
/// optional reading-progress slider, and a spoiler-syntax hint. The open
/// composer's markup is composed from [`BdJournalComposerTabs`],
/// [`BdJournalHighlightsPopover`], [`BdJournalEditorBody`], and
/// [`BdJournalComposerFoot`].
#[component]
fn BdJournalComposer(uuid: String, server_url: String, reload: Signal<u32>) -> Element {
    let mut open = use_signal(|| false);
    let body = use_signal(String::new);
    let show_preview = use_signal(|| false);
    let preview_html = use_signal(String::new);
    let track_progress = use_signal(|| false);
    let progress = use_signal(|| 50i64);
    let saving = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    // Owned here (not inside `BdJournalHighlightsPopover`) because the
    // popover's markup is conditionally skipped while `show_preview` is true —
    // if the popover owned this state itself, toggling to Preview and back
    // would unmount/remount it and drop the loaded-once cache, forcing a
    // needless highlights refetch.
    let highlights = use_signal(Vec::<Highlight>::new);
    let highlights_open = use_signal(|| false);
    let highlights_loaded = use_signal(|| false);

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
            BdJournalComposerTabs {
                server_url: server_url.clone(),
                body,
                show_preview,
                preview_html,
                error,
            }
            if !show_preview() {
                div { class: "bd-journal-toolbar-row",
                    BdJournalToolbar { target_id: "journal-composer-editor".to_string() }
                    BdJournalHighlightsPopover {
                        uuid: uuid.clone(),
                        server_url: server_url.clone(),
                        highlights,
                        highlights_open,
                        highlights_loaded,
                    }
                }
            }
            BdJournalEditorBody { show_preview, preview_html, body }
            BdJournalComposerFoot {
                uuid,
                server_url,
                reload,
                state: JournalComposerState {
                    open,
                    body,
                    track_progress,
                    progress,
                    saving,
                    error,
                    show_preview,
                },
            }
        }
    }
}

/// Write/Preview tab row + spoiler-syntax hint above the composer body.
/// Switching to "Preview" fetches a server-rendered markdown preview of the
/// current draft.
#[component]
fn BdJournalComposerTabs(
    server_url: String,
    body: Signal<String>,
    show_preview: Signal<bool>,
    preview_html: Signal<String>,
    error: Signal<Option<String>>,
) -> Element {
    let mut show_preview = show_preview;
    let mut preview_html = preview_html;
    let mut error = error;
    rsx! {
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
                onclick: move |_| {
                    let url = server_url.clone();
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
    }
}

/// "From highlights" toggle + popover, letting the composer insert a saved
/// highlight as a blockquote at the caret. Highlights load lazily the first
/// time the popover opens. The loaded-once state is owned by
/// `BdJournalComposer` (not here) because this component's markup is
/// conditionally skipped while previewing — owning it locally would drop the
/// cache and refetch every time the write/preview tabs are toggled.
#[component]
fn BdJournalHighlightsPopover(
    uuid: String,
    server_url: String,
    highlights: Signal<Vec<Highlight>>,
    highlights_open: Signal<bool>,
    highlights_loaded: Signal<bool>,
) -> Element {
    let mut highlights = highlights;
    let mut highlights_open = highlights_open;
    let mut highlights_loaded = highlights_loaded;

    rsx! {
        div { class: "bd-journal-hl-wrap",
            button {
                r#type: "button",
                class: if highlights_open() { "btn ghost sm bd-tab-active" } else { "btn ghost sm" },
                "data-testid": "journal-insert-highlight",
                onclick: move |_| {
                    let opening = !highlights_open();
                    highlights_open.set(opening);
                    if opening && !highlights_loaded() {
                        highlights_loaded.set(true);
                        let url = server_url.clone();
                        let uuid = uuid.clone();
                        spawn(async move {
                            if let Ok(list) = data::list_highlights(&url, &uuid).await {
                                highlights
                                    .set(list.into_iter().filter(|h| h.text.is_some()).collect());
                            }
                        });
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
                                key: "{h.id}",
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

/// The write/preview surface. Preview mode renders sanitized server HTML;
/// write mode pairs a plain textarea (SSR / first-paint source of truth) with
/// a progressively enhanced contenteditable div — rule 07 hydration parity:
/// SSR and first-hydration paint both show the plain textarea, and post-mount
/// JS flips the wrapper's enhancement marker (see `editor_enhance`). The
/// textarea keeps carrying the `body` signal even after enhancement — the JS
/// mirrors the editor's markdown back into its `value`, so
/// publish/validate/preview are unchanged.
#[component]
fn BdJournalEditorBody(
    show_preview: Signal<bool>,
    preview_html: Signal<String>,
    body: Signal<String>,
) -> Element {
    let mut body = body;
    rsx! {
        if show_preview() {
            div {
                class: "bd-journal-preview",
                "data-testid": "journal-preview",
                dangerous_inner_html: "{preview_html}",
            }
        } else {
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
    }
}

/// Reading-progress checkbox + slider shown in the composer footer while an
/// entry is being written.
#[component]
fn BdJournalProgressToggle(track_progress: Signal<bool>, progress: Signal<i64>) -> Element {
    let mut track_progress = track_progress;
    let mut progress = progress;
    rsx! {
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
    }
}

/// Composer footer: reading-progress toggle/slider, inline error, cancel, and
/// publish. Publishing posts the entry, then resets and closes the composer.
#[component]
fn BdJournalComposerFoot(
    uuid: String,
    server_url: String,
    reload: Signal<u32>,
    state: JournalComposerState,
) -> Element {
    let JournalComposerState {
        open,
        body,
        mut track_progress,
        progress,
        saving,
        error,
        show_preview,
    } = state;
    let mut reload = reload;
    let mut open = open;
    let mut body = body;
    let mut saving = saving;
    let mut error = error;
    let mut show_preview = show_preview;

    rsx! {
        div { class: "bd-journal-composer-foot",
            BdJournalProgressToggle { track_progress, progress }
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
                onclick: move |_| {
                    let url = server_url.clone();
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
                },
                if saving() { "Publishing\u{2026}" } else { "Publish entry" }
            }
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
fn BdJournalEntryCard(
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
                if !is_owner {
                    if let Some(msg) = error() {
                        span { class: "mono bd-journal-error", role: "alert", "{msg}" }
                    }
                }
            }
        }
    }
}

/// Markdown toolbar + textarea/contenteditable pair for editing an existing
/// entry (same progressive-enhancement pattern as the composer — see
/// `BdJournalEditorBody`), keyed per entry id so multiple open editors don't
/// collide.
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
