//! Book detail page — the marquee stage on web, single-column re-flow on
//! mobile.
//!
//! Owns the data fetch and shared markup primitives; the loaded-book view
//! composition lives in [`view`], the web stage in [`marquee`] and its stop
//! submodules, and shared discovery sections in [`body`].

use dioxus::prelude::*;
use omnibus_shared::physical::WishlistEntry;
use omnibus_shared::{EbookMetadata, MergeBooksResult, SuggestionsResponse};

use crate::components::{PageError, PageLoading, PageNotFound};
use crate::{data, use_server_url, Route};

mod dates;
mod delete;
mod discovery;
mod file_picker;
mod immersive;
mod journal;
mod journal_editor;
mod merge;
mod rating;
mod read_status;
#[cfg(not(feature = "mobile"))]
mod sync_link;
mod view;

// Web renders the marquee stage (seven snap-scrolled stops); mobile
// re-flows the same loaded-book data into a single-column surface. The
// web-only sections aren't compiled on the native shell. (Separate build →
// no impact on web SSR/WASM parity, rule 07.)
#[cfg(not(feature = "mobile"))]
mod body;
#[cfg(not(feature = "mobile"))]
mod chips;
#[cfg(not(feature = "mobile"))]
mod export_menu;
#[cfg(not(feature = "mobile"))]
mod highlights;
#[cfg(not(feature = "mobile"))]
mod marquee;
#[cfg(feature = "mobile")]
mod mobile;
#[cfg(feature = "mobile")]
mod offline;
#[cfg(not(feature = "mobile"))]
mod physical;

// The landing Continue hero's Immersive pill shares the book-detail CTA's
// retarget-and-navigate handler; the hero is web-only, so the re-export is
// gated with it (the mobile CTA calls the fn inside `immersive` directly).
#[cfg(not(feature = "mobile"))]
pub(crate) use immersive::retarget_and_open_immersive;
use view::{render_loaded, LoadedCtx, RailButtons};
// Only `mobile.rs` (compiled solely under this feature) re-derives the loaded
// view itself; the web branch calls `derive_loaded_view` directly inside
// `view::render_loaded`, so this re-export would otherwise be unused there.
#[cfg(feature = "mobile")]
use view::{derive_loaded_view, LoadedBookView};

/// Book detail page shell: fetches metadata then hands off to `render_loaded`.
#[component]
pub fn BookDetailPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let sig = use_book_detail_signals();
    // Reflect the loaded book title in the browser tab (e.g. "Omnibus | Dracula").
    let book = sig.book;
    crate::use_page_title(move || book.read().as_ref().and_then(|b| b.title.clone()));

    use_book_data_effects(
        uuid.clone(),
        server_url.clone(),
        BookDataSignals {
            book: sig.book,
            author_books: sig.author_books,
            loading: sig.loading,
            error: sig.error,
            refresh: sig.merge.refresh,
            suggestions_epoch: sig.suggestions_epoch,
            suggestions: sig.suggestions,
            description: sig.description,
        },
    );

    if (sig.loading)() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = (sig.error)() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }
    let Some(b) = (sig.book)() else {
        return rsx! { PageNotFound { subject: "Book", back_to: Route::Landing {} } };
    };

    render_book_shell(
        b,
        sig.merge,
        PageSignals {
            description: sig.description,
            delete_open: sig.delete_open,
            phys: sig.phys,
        },
        (sig.author_books)(),
        (sig.suggestions)(),
        sig.is_admin,
        server_url,
    )
}

/// Every signal [`BookDetailPage`] owns, declared unconditionally (rule 07)
/// so the hook count and order stay identical on every platform target.
/// Bundled into one custom-hook-style helper — called unconditionally, so
/// Dioxus's per-scope hook-order tracking sees the same sequence of
/// `use_signal` calls every render — so the component body reads as a
/// data-fetch + dispatch shell instead of a wall of hook declarations.
struct BookDetailSignals {
    book: Signal<Option<EbookMetadata>>,
    author_books: Signal<Vec<EbookMetadata>>,
    // F3.3 suggestions. Starts `None` on both SSR and the first WASM paint so
    // hydration markup matches (rule 07); the client effect below populates it.
    suggestions: Signal<Option<SuggestionsResponse>>,
    // Epoch guard so a poll loop left over from a previous book can't write
    // its result onto the current book after navigation (mirrors landing's
    // `fetch_epoch`).
    suggestions_epoch: Signal<u64>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    // Admin gating for the "Merge with…" affordance — derived from the
    // app-wide `CurrentUser` context instead of an independent per-mount
    // `/api/auth/me` fetch. Mobile/SSR stay at the `false` default since the
    // context is web-only; the server-side `AdminUser` extractor on
    // `rpc_merge_books` is the actual security boundary.
    is_admin: ReadSignal<bool>,
    // Merge-dialog state; mobile compiles the signals but `render_book_shell`
    // stubs the builders that read them.
    merge: MergeSignals,
    // Delete-dialog state, declared unconditionally for the same reason.
    delete_open: Signal<bool>,
    // Physical-wishlist state, lifted here because two web sections share it:
    // the hero (cover chip + rail wishlist slot) and the physical panel
    // (whose post-mount load populates it, and whose last-copy delete can
    // write it). Mobile discards it.
    phys: PhysSignals,
    // Fetch Summary override (mobile only); reached only past the early
    // returns below. `epoch` bumps alongside `value` on every fresh book
    // load, so the Fetch Summary save in `render_loaded_mobile` can detect
    // and drop a result left over from a since-navigated-away book.
    description: DescriptionSignals,
}

fn use_book_detail_signals() -> BookDetailSignals {
    let refresh = use_signal(|| 0u32);
    BookDetailSignals {
        book: use_signal(|| None),
        author_books: use_signal(Vec::new),
        suggestions: use_signal(|| None),
        suggestions_epoch: use_signal(|| 0u64),
        loading: use_signal(|| true),
        error: use_signal(|| None),
        is_admin: crate::use_is_admin(),
        merge: MergeSignals {
            open: use_signal(|| false),
            result: use_signal(|| None),
            undo_error: use_signal(|| None),
            refresh,
            after_merge: use_signal(|| false),
        },
        delete_open: use_signal(|| false),
        phys: PhysSignals {
            wishlist: use_signal(|| None::<WishlistEntry>),
            loaded: use_signal(|| false),
        },
        description: DescriptionSignals {
            value: use_signal(String::new),
            epoch: use_signal(|| 0u64),
        },
    }
}

/// Fetch Summary override signals (mobile only): the effective description
/// plus the epoch that guards a pending fetch-and-save against resolving
/// onto a different, later-navigated book. `Copy` (Dioxus signals) and
/// grouped so call sites stay under the function-argument cap.
#[derive(Clone, Copy)]
struct DescriptionSignals {
    value: Signal<String>,
    epoch: Signal<u64>,
}

/// Extra per-page state threaded from [`BookDetailPage`]'s prologue into
/// [`render_book_shell`]: the Fetch Summary override (mobile) and the
/// delete-dialog open toggle (web). Grouped only to keep that call site
/// under clippy's argument cap. `Copy` (Dioxus signals), so passing by
/// value is cheap.
#[derive(Clone, Copy)]
struct PageSignals {
    description: DescriptionSignals,
    delete_open: Signal<bool>,
    phys: PhysSignals,
}

/// Shared physical-wishlist state: the entry itself plus the loaded flag for
/// the panel's post-mount copies+wishlist fetch. Both the hero (chip + rail
/// slot) and the physical panel hold the same signals, so a mutation in either
/// place is immediately visible in the other.
#[derive(Clone, Copy, PartialEq)]
struct PhysSignals {
    wishlist: Signal<Option<WishlistEntry>>,
    loaded: Signal<bool>,
}

/// The book-page data signals written by the fetch effects. `Copy` (Dioxus
/// signals), so the group is passed by value.
#[derive(Clone, Copy)]
struct BookDataSignals {
    book: Signal<Option<EbookMetadata>>,
    author_books: Signal<Vec<EbookMetadata>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    refresh: Signal<u32>,
    suggestions_epoch: Signal<u64>,
    suggestions: Signal<Option<SuggestionsResponse>>,
    description: DescriptionSignals,
}

/// Install the two `uuid`-reactive fetch effects: the book + author-books load
/// (also re-armed by `refresh` after a merge/undo) and the suggestions
/// poll. Called unconditionally from [`BookDetailPage`] so the hook sequence
/// stays fixed; both effects re-run when `uuid` changes.
fn use_book_data_effects(uuid: String, server_url: String, sig: BookDataSignals) {
    let url = server_url.clone();
    let refresh = sig.refresh;
    let generation = crate::use_cache_generation();
    use_effect(use_reactive!(|uuid| {
        // Read `refresh` so a merge/undo bump re-arms this effect, and the
        // cache generation so a background revalidation refreshes the page
        // (served from the fresh cache, zero network).
        let _ = refresh();
        let _ = generation();
        fetch_book_and_author_books(
            url.clone(),
            uuid.clone(),
            sig.book,
            sig.author_books,
            sig.loading,
            sig.error,
            sig.description,
        );
    }));

    // F3.3 suggestions: resolved off-request by the worker and cached. Fetch
    // for the current book; while the result is `Pending`, poll a few times on
    // web so it appears without a manual reload. A fetch failure degrades to
    // "no suggestions" rather than an error row.
    let sug_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        poll_suggestions_until_resolved(
            sug_url.clone(),
            uuid.clone(),
            sig.suggestions_epoch,
            sig.suggestions,
        );
    }));
}

/// Merge-dialog signals threaded from [`BookDetailPage`] into the loaded-book
/// renderer. `Copy` (Dioxus signals), so passing them by value is cheap and
/// keeps the callee free of extra hook calls.
#[derive(Clone, Copy)]
struct MergeSignals {
    open: Signal<bool>,
    result: Signal<Option<MergeBooksResult>>,
    undo_error: Signal<Option<String>>,
    refresh: Signal<u32>,
    /// One-shot: set by a successful merge so the alignment panel can
    /// offer linking the freshly dual-format book.
    after_merge: Signal<bool>,
}

/// Assemble the loaded-book view: derive the admin flag, build the (web-only)
/// merge button + dialog, and compose them with [`render_loaded`]. Kept a plain
/// fn so the platform `cfg` gates live here instead of the component body
/// (rule 07 — no new gates inside `BookDetailPage`).
fn render_book_shell(
    b: EbookMetadata,
    merge: MergeSignals,
    page: PageSignals,
    author_books: Vec<EbookMetadata>,
    suggestions: Option<SuggestionsResponse>,
    is_admin: ReadSignal<bool>,
    server_url: String,
) -> Element {
    let PageSignals {
        description,
        delete_open,
        phys,
    } = page;

    // `is_admin` starts at `false` and only flips to `true` on the web
    // client after `current_user()` resolves an admin user, so this read
    // returns `false` during SSR and for non-admins on every platform.
    let is_admin_flag = is_admin();

    let (merge_button, merge_ui) = build_merge_pieces(is_admin_flag, merge, &server_url, &b);
    let (delete_button, delete_ui) =
        build_delete_pieces(is_admin_flag, delete_open, merge.refresh, &b);

    let body = render_loaded(
        b,
        description,
        author_books,
        RailButtons {
            merge: merge_button,
            delete: delete_button,
        },
        suggestions,
        LoadedCtx {
            server_url,
            is_admin: is_admin_flag,
            refresh: merge.refresh,
            phys,
            after_merge: merge.after_merge,
        },
    );
    rsx! {
        {body}
        {merge_ui}
        {delete_ui}
    }
}

/// Rail "Merge with…" button (admin, web only) plus its dialog, threaded down
/// as prebuilt `Element`s so the rail component stays platform-agnostic.
fn build_merge_pieces(
    is_admin_flag: bool,
    merge: MergeSignals,
    server_url: &str,
    b: &EbookMetadata,
) -> (Option<Element>, Option<Element>) {
    #[cfg(not(feature = "mobile"))]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag, merge.open);
    #[cfg(feature = "mobile")]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag);

    #[cfg(not(feature = "mobile"))]
    let merge_ui: Option<Element> = merge::build_merge_ui(
        merge.open,
        merge.result,
        merge.undo_error,
        merge.refresh,
        merge.after_merge,
        server_url.to_string(),
        b.clone(),
    );
    #[cfg(feature = "mobile")]
    let merge_ui: Option<Element> = {
        // Mobile's builders take no args; read every field (plus the
        // web-only params) so the signals — declared unconditionally in
        // `BookDetailPage` for hook parity — aren't flagged as never-read
        // on this target.
        let MergeSignals {
            open,
            result,
            undo_error,
            refresh,
            after_merge,
        } = merge;
        let _ = (
            open,
            result,
            undo_error,
            refresh,
            after_merge,
            server_url,
            b,
        );
        merge::build_merge_ui()
    };

    (merge_button, merge_ui)
}

/// Rail "Delete files…" button + its dialog, same web-only shape as
/// [`build_merge_pieces`].
fn build_delete_pieces(
    is_admin_flag: bool,
    delete_open: Signal<bool>,
    refresh: Signal<u32>,
    b: &EbookMetadata,
) -> (Option<Element>, Option<Element>) {
    #[cfg(not(feature = "mobile"))]
    let delete_button: Option<Element> = delete::build_delete_button(is_admin_flag, delete_open);
    #[cfg(feature = "mobile")]
    let delete_button: Option<Element> = delete::build_delete_button(is_admin_flag);

    #[cfg(not(feature = "mobile"))]
    let delete_ui: Option<Element> = delete::build_delete_ui(
        delete_open,
        refresh,
        b.unique_identifier.clone().unwrap_or_default(),
        b.display_title(),
    );
    #[cfg(feature = "mobile")]
    let delete_ui: Option<Element> = {
        let _ = (delete_open, refresh, b);
        delete::build_delete_ui()
    };

    (delete_button, delete_ui)
}

// View helpers — the loaded-book case is the only thing rendered, so the
// data-fetch shell above stays small.

/// Fetch the book, then (if it resolves and has a creator) the primary
/// author's other books, filtering out the current book. `book` and
/// `author_books` are written directly so a fast re-run (uuid change) is
/// naturally superseded by the newer fetch's writes. `description.value`
/// (mobile's Fetch Summary override) resets to the freshly-loaded book's
/// saved summary on every call, and `description.epoch` bumps alongside it
/// so a fetch-and-save left over from a previous book can detect it's stale
/// and drop its result instead of overwriting the new book's description.
fn fetch_book_and_author_books(
    server_url: String,
    uuid: String,
    mut book: Signal<Option<EbookMetadata>>,
    mut author_books: Signal<Vec<EbookMetadata>>,
    mut loading: Signal<bool>,
    mut error: Signal<Option<String>>,
    mut description: DescriptionSignals,
) {
    spawn(async move {
        // Keep the rendered page in place when re-fetching the *same* book
        // (a cache-revalidation bump) — the skeleton and author-rail reset
        // are for actual book changes. `unique_identifier` is the book uuid
        // on API rows.
        let same_book = book
            .peek()
            .as_ref()
            .and_then(|b| b.unique_identifier.as_deref())
            == Some(uuid.as_str());
        if !same_book {
            loading.set(true);
            author_books.set(Vec::new());
        }
        match data::get_ebook(&server_url, &uuid).await {
            Ok(b) => {
                let author_fetch = b.as_ref().map(|inner| {
                    (
                        inner.creators.first().and_then(|c| c.id),
                        inner.unique_identifier.clone(),
                    )
                });
                description.value.set(
                    b.as_ref()
                        .and_then(|inner| inner.description.clone())
                        .unwrap_or_default(),
                );
                description.epoch.with_mut(|e| *e += 1);
                book.set(b);
                error.set(None);
                loading.set(false);
                if let Some((Some(aid), current_uuid)) = author_fetch {
                    if let Ok(Some(ad)) = data::get_author(&server_url, aid).await {
                        let still_current =
                            book().as_ref().and_then(|b| b.unique_identifier.as_ref())
                                == current_uuid.as_ref();
                        if still_current {
                            let others: Vec<EbookMetadata> = ad
                                .books
                                .into_iter()
                                .filter(|ab| ab.unique_identifier != current_uuid)
                                .collect();
                            author_books.set(others);
                        }
                    }
                }
            }
            Err(e) => {
                error.set(Some(e.to_string()));
                loading.set(false);
            }
        }
    });
}

/// Fetch suggestions for `uuid` and, on web, poll a few times while the
/// result is `Pending`. `suggestions_epoch` is bumped once per call and
/// captured as the run's identity, so a stale poll left over from a previous
/// book (fast SPA navigation) drops its result instead of overwriting the
/// now-current book's suggestions.
fn poll_suggestions_until_resolved(
    server_url: String,
    uuid: String,
    mut suggestions_epoch: Signal<u64>,
    mut suggestions: Signal<Option<SuggestionsResponse>>,
) {
    let epoch = {
        suggestions_epoch.with_mut(|e| *e += 1);
        *suggestions_epoch.peek()
    };
    let is_current = move || *suggestions_epoch.peek() == epoch;
    spawn(async move {
        suggestions.set(None);
        match data::get_suggestions(&server_url, &uuid).await {
            Ok(resp) => {
                if !is_current() {
                    return;
                }
                let pending = matches!(resp, SuggestionsResponse::Pending);
                suggestions.set(Some(resp));
                #[cfg(feature = "web")]
                if pending {
                    let mut tries = 0u32;
                    while tries < 5 {
                        gloo_timers::future::TimeoutFuture::new(2500).await;
                        if !is_current() {
                            return;
                        }
                        tries += 1;
                        match data::get_suggestions(&server_url, &uuid).await {
                            Ok(next) => {
                                if !is_current() {
                                    return;
                                }
                                let still_pending = matches!(next, SuggestionsResponse::Pending);
                                suggestions.set(Some(next));
                                if !still_pending {
                                    break;
                                }
                            }
                            Err(_) => {
                                // Mirror the initial-fetch error path below: fall
                                // back to the empty-ready state so the UI doesn't
                                // get stuck on "loading suggestions" forever.
                                if is_current() {
                                    suggestions.set(Some(SuggestionsResponse::Ready {
                                        items: Vec::new(),
                                    }));
                                }
                                break;
                            }
                        }
                    }
                }
                #[cfg(not(feature = "web"))]
                let _ = pending;
            }
            Err(_) => {
                if is_current() {
                    suggestions.set(Some(SuggestionsResponse::Ready { items: Vec::new() }));
                }
            }
        }
    });
}

// Page-local primitives. None of these introduce business logic — they're
// markup-only adapters so the page reads as a composition of named blocks
// rather than nested rsx.

/// Body section heading row — kicker label + serif title, with an optional
/// right-aligned `action` slot (mirrors `screens/_shared.jsx#SectionHead`). The
/// kicker stacks above the title; the action floats opposite via the flex
/// `space-between` on `.bd-section-head`.
#[component]
pub(super) fn BdSectionHead(
    kicker: String,
    title: String,
    #[props(default)] action: Option<Element>,
) -> Element {
    rsx! {
        div { class: "bd-section-head",
            div { class: "bd-section-head-text",
                if !kicker.is_empty() {
                    div { class: "label bd-section-kicker", "{kicker}" }
                }
                h3 { class: "bd-section-title", "{title}" }
            }
            {action}
        }
    }
}

/// Small monospace format pill (matches `screens/_shared.jsx#FormatBadge`).
#[component]
pub(super) fn BdFormatBadge(fmt: String) -> Element {
    rsx! {
        div { class: "bd-fmt-badge", "data-testid": "bd-format-badge", "{fmt}" }
    }
}

#[component]
pub(super) fn BdMetaRow(k: String, v: String) -> Element {
    rsx! {
        tr { class: "bd-meta-row",
            td { class: "bd-meta-k", "{k}" }
            td { class: "bd-meta-v", "{v}" }
        }
    }
}
