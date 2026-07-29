//! The reading-journal draft composer: collapsed prompt → expanded markdown
//! editor with a Write/Preview toggle, a "from highlights" insert popover, an
//! optional reading-progress slider, and the Publish/Cancel footer.

use dioxus::core::Task;
use dioxus::prelude::*;
use omnibus_shared::{CreateJournalEntry, Highlight, JournalStatus, UpdateJournalEntry};

use crate::data;
use crate::pages::book_detail::journal_editor::*;
use crate::platform_sleep::async_sleep_ms;

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
    /// Id of the server-side draft row the composer is riding (created by the
    /// first autosave or Save-draft click); publish/cancel resolve it.
    draft_id: Signal<Option<i64>>,
    /// In-flight debounced autosave, cancelled per keystroke (search-palette
    /// pattern) and before any explicit save/publish/cancel.
    autosave_task: Signal<Option<Task>>,
    /// Seconds since the last successful autosave — `Some(0)` right after a
    /// save, bumped by [`JournalComposerState::autosaved_ticker`]; `None`
    /// until the first autosave lands.
    autosaved_secs: Signal<Option<u64>>,
    /// The ticking task behind the "Auto-saved Ns ago" indicator.
    autosaved_ticker: Signal<Option<Task>>,
}

impl JournalComposerState {
    /// Cancel pending autosave work and reset every composer signal back to
    /// the collapsed-prompt state.
    fn reset_and_close(mut self) {
        if let Some(t) = self.autosave_task.write().take() {
            t.cancel();
        }
        if let Some(t) = self.autosaved_ticker.write().take() {
            t.cancel();
        }
        self.draft_id.set(None);
        self.autosaved_secs.set(None);
        self.body.set(String::new());
        self.show_preview.set(false);
        self.track_progress.set(false);
        self.error.set(None);
        self.open.set(false);
    }

    /// The optional progress payload derived from the footer toggle/slider.
    fn progress_payload(&self) -> Option<u8> {
        if (self.track_progress)() {
            Some((self.progress)() as u8)
        } else {
            None
        }
    }
}

/// Collapsed prompt → expanded markdown composer with a Write/Preview toggle, an
/// optional reading-progress slider, and a spoiler-syntax hint. The open
/// composer's markup is composed from [`BdJournalComposerTabs`],
/// [`BdJournalHighlightsPopover`], [`BdJournalEditorBody`], and
/// [`BdJournalComposerFoot`].
#[component]
pub(super) fn BdJournalComposer(uuid: String, server_url: String, reload: Signal<u32>) -> Element {
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
    let draft_id = use_signal(|| None::<i64>);
    let mut autosave_task = use_signal(|| None::<Task>);
    let autosaved_secs = use_signal(|| None::<u64>);
    let autosaved_ticker = use_signal(|| None::<Task>);

    let state = JournalComposerState {
        open,
        body,
        track_progress,
        progress,
        saving,
        error,
        show_preview,
        draft_id,
        autosave_task,
        autosaved_secs,
        autosaved_ticker,
    };

    // Debounced autosave: each body change cancels the pending task and arms a
    // fresh one (search-palette pattern), so at most one save is queued and a
    // typing burst produces a single write once the user pauses.
    let autosave_url = server_url.clone();
    let autosave_uuid = uuid.clone();
    use_effect(move || {
        let text = body();
        if !open() || saving() || text.trim().is_empty() {
            return;
        }
        if let Some(prev) = autosave_task.write().take() {
            prev.cancel();
        }
        let url = autosave_url.clone();
        let uuid = autosave_uuid.clone();
        let task = spawn(async move {
            async_sleep_ms(AUTOSAVE_DEBOUNCE_MS).await;
            autosave_draft(&url, &uuid, text, state).await;
        });
        autosave_task.set(Some(task));
    });

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
                    BdJournalToolbar {
                        target_id: "journal-composer-editor".to_string(),
                        server_url: server_url.clone(),
                        error,
                    }
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
                state,
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
                {
                    let pct = progress();
                    rsx! {
                        div { class: "bd-journal-progress-slider",
                            input {
                                r#type: "range",
                                class: "bd-journal-progress-range",
                                min: "0",
                                max: "100",
                                value: "{progress}",
                                // Paints the played portion directly (Chrome/Safari drop
                                // native fill once `appearance: none` is set), so drag
                                // affordance stays visible on touch, not just the thumb.
                                style: "background: linear-gradient(to right, var(--accent) 0%, var(--accent) {pct}%, var(--bg-3) {pct}%, var(--bg-3) 100%);",
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
        }
    }
}

/// Debounce window between the last keystroke and the autosave write.
const AUTOSAVE_DEBOUNCE_MS: u32 = 2_000;

/// How often the "Auto-saved Ns ago" indicator ticks forward.
const AUTOSAVE_TICK_MS: u32 = 5_000;

/// Persist the composer body as a per-user draft: the first save creates the
/// draft row (and starts the indicator ticker), later saves update it in
/// place. Failures are silent — the explicit Save-draft/Publish paths surface
/// errors, and the next keystroke re-arms the autosave anyway.
async fn autosave_draft(url: &str, uuid: &str, body_md: String, state: JournalComposerState) {
    let mut state = state;
    let saved_id = match (state.draft_id)() {
        Some(id) => data::update_journal_entry(
            url,
            id,
            UpdateJournalEntry {
                body_md,
                progress: state.progress_payload(),
                status: None,
            },
        )
        .await
        .map(|e| e.id),
        None => data::create_journal_entry(
            url,
            CreateJournalEntry {
                client_id: None,
                book_uuid: uuid.to_string(),
                body_md,
                progress: state.progress_payload(),
                status: JournalStatus::Draft,
            },
        )
        .await
        .map(|e| e.id),
    };
    let Ok(id) = saved_id else { return };
    state.draft_id.set(Some(id));
    state.autosaved_secs.set(Some(0));
    if state.autosaved_ticker.peek().is_none() {
        let mut secs = state.autosaved_secs;
        let ticker = spawn(async move {
            loop {
                async_sleep_ms(AUTOSAVE_TICK_MS).await;
                let current = *secs.peek();
                if let Some(s) = current {
                    secs.set(Some(s + u64::from(AUTOSAVE_TICK_MS / 1_000)));
                }
            }
        });
        state.autosaved_ticker.set(Some(ticker));
    }
}

/// Composer footer: reading-progress toggle/slider, the "Auto-saved Ns ago"
/// indicator, inline error, cancel, save-draft, and publish. Save draft keeps
/// the entry owner-private; publish surfaces it in the shared feed. Both
/// reset and close the composer; cancel additionally discards any autosaved
/// draft.
#[component]
fn BdJournalComposerFoot(
    uuid: String,
    server_url: String,
    reload: Signal<u32>,
    state: JournalComposerState,
) -> Element {
    let JournalComposerState {
        body,
        saving,
        error,
        autosaved_secs,
        draft_id,
        ..
    } = state;
    let mut reload = reload;
    let mut saving = saving;
    let mut error = error;

    // Save the current body as `status`, then reset/close/reload. Shared by
    // the Save-draft and Publish buttons — the only difference is the status
    // the entry ends up in (an existing autosaved draft row is reused).
    let save_url = server_url.clone();
    let save_uuid = uuid.clone();
    let mut save_as = move |status: JournalStatus| {
        let url = save_url.clone();
        let uuid = save_uuid.clone();
        let mut state = state;
        if let Some(t) = state.autosave_task.write().take() {
            t.cancel();
        }
        saving.set(true);
        error.set(None);
        spawn(async move {
            let result = match draft_id() {
                Some(id) => data::update_journal_entry(
                    &url,
                    id,
                    UpdateJournalEntry {
                        body_md: body(),
                        progress: state.progress_payload(),
                        status: Some(status),
                    },
                )
                .await
                .map(|_| ()),
                None => data::create_journal_entry(
                    &url,
                    CreateJournalEntry {
                        client_id: None,
                        book_uuid: uuid,
                        body_md: body(),
                        progress: state.progress_payload(),
                        status,
                    },
                )
                .await
                .map(|_| ()),
            };
            match result {
                Ok(()) => {
                    state.reset_and_close();
                    reload.set(reload() + 1);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "bd-journal-composer-foot",
            BdJournalProgressToggle {
                track_progress: state.track_progress,
                progress: state.progress,
            }
            span { class: "bd-journal-foot-spacer" }
            if let Some(secs) = autosaved_secs() {
                span {
                    class: "mono bd-journal-autosaved",
                    "data-testid": "journal-autosaved",
                    if secs == 0 { "Auto-saved just now" } else { "Auto-saved {secs}s ago" }
                }
            }
            if let Some(msg) = error() {
                span { class: "mono bd-journal-error", role: "alert", "{msg}" }
            }
            button {
                r#type: "button",
                class: "btn ghost sm",
                disabled: saving(),
                onclick: move |_| {
                    let url = server_url.clone();
                    if let Some(t) = state.autosave_task.write().take() {
                        t.cancel();
                    }
                    saving.set(true);
                    spawn(async move {
                        // Discard any autosaved draft row (best-effort) BEFORE
                        // closing — reset_and_close() unmounts this scope, and
                        // a task spawned after unmount is dropped unpolled.
                        if let Some(id) = draft_id() {
                            let _ = data::delete_journal_entry(&url, id).await;
                        }
                        saving.set(false);
                        state.reset_and_close();
                    });
                },
                "Cancel"
            }
            button {
                r#type: "button",
                class: "btn ghost sm",
                "data-testid": "journal-save-draft",
                disabled: saving() || body().trim().is_empty(),
                onclick: {
                    let mut save_as = save_as.clone();
                    move |_| save_as(JournalStatus::Draft)
                },
                if saving() { "Saving\u{2026}" } else { "Save draft" }
            }
            button {
                r#type: "button",
                class: "btn primary sm",
                "data-testid": "journal-publish",
                disabled: saving() || body().trim().is_empty(),
                onclick: move |_| save_as(JournalStatus::Published),
                if saving() { "Publishing\u{2026}" } else { "Publish entry" }
            }
        }
    }
}
