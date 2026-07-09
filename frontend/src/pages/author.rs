//! Author discovery page — shows author details and their books grouped by
//! series, mirroring the `AuthorScreen` design comp from
//! `screens/discovery.jsx`.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;
use dioxus_router::Link;
use omnibus_shared::{AuthorDetail, EbookMetadata};

use crate::components::atrium::Cover;
use crate::components::author_photo_edit::AuthorPhotoEditOverlay;
use crate::components::{PageError, PageLoading, PageNotFound};
use crate::{data, use_server_url, Route};

/// Renders the author discovery page.
#[component]
pub fn AuthorPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut author: Signal<Option<AuthorDetail>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // F5.9-lite admin gating for the Delete button — derived from the
    // app-wide `CurrentUser` context (`crate::use_is_admin`) instead of an
    // independent per-mount `/api/auth/me` fetch. Mobile/SSR stay at the
    // `false` default since the context is web-only; the server-side
    // `AdminUser` extractor on `rpc_delete_author` is the actual security
    // boundary.
    let is_admin = crate::use_is_admin();

    // See `BookDetailPage` for why `id` needs `use_reactive!`.
    let url = server_url.clone();
    use_effect(use_reactive!(|id| {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::get_author(&url, id).await {
                Ok(a) => {
                    author.set(a);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    }));

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }
    let Some(a) = author() else {
        return rsx! { PageNotFound { subject: "Author", back_to: Route::Landing {} } };
    };

    // `is_admin` starts at `false` and only flips to `true` after the
    // effect above resolves against the client-side session. That keeps
    // the first-hydration paint identical to SSR (no Delete affordance)
    // before the client reconciles with the real admin flag.
    let is_admin_flag = is_admin();
    render_author(a, server_url, author, is_admin_flag)
}

// allow: ~86 lines — splitting further means threading the mobile-gated admin-delete signals across a function boundary that doesn't exist on mobile builds.
fn render_author(
    a: AuthorDetail,
    server_url: String,
    author: Signal<Option<AuthorDetail>>,
    #[cfg_attr(feature = "mobile", allow(unused_variables))] is_admin: bool,
) -> Element {
    // Derive accent from the first book that has one, or fall back to theme.
    let accent = a
        .books
        .iter()
        .find_map(|b| b.accent.as_deref())
        .unwrap_or("var(--accent)");
    let (first, last) = split_name(&a.name);
    let initial = author_initial(&a.name);
    let (series_groups, standalone) = group_books_by_series(&a.books);
    let bg_style = format!(
        "radial-gradient(50% 80% at 80% 20%, color-mix(in oklch, {accent} 14%, transparent), transparent 70%)"
    );

    // F5.9-lite (issue #159) admin Delete-author state. The button is
    // gated by `is_admin` server-side via `AdminUser` on
    // `rpc_delete_author`, but we also hide the affordance entirely
    // for non-admin viewers. Web-only — the mobile build never
    // references these signals so we don't allocate them at all.
    #[cfg(not(feature = "mobile"))]
    let show_confirm = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let deleting = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let delete_error: Signal<Option<String>> = use_signal(|| None);

    // F5.9-lite admin Delete affordance is web-only — the matching
    // `data::delete_author` server fn is gated `not(feature = "mobile")`
    // per the F5.9-lite plan's "admin-web only" v1 scope.
    let admin_actions = {
        #[cfg(not(feature = "mobile"))]
        let admin_actions = is_admin.then(|| {
            rsx! {
                div { class: "author-admin-actions",
                    button {
                        class: "btn author-delete-btn",
                        "data-testid": "author-delete-btn",
                        onclick: {
                            let mut show_confirm = show_confirm;
                            let mut delete_error = delete_error;
                            move |_| {
                                delete_error.set(None);
                                show_confirm.set(true);
                            }
                        },
                        "Delete author"
                    }
                }
            }
        });
        #[cfg(feature = "mobile")]
        let admin_actions: Option<Element> = None;
        admin_actions
    };

    let modal = {
        #[cfg(not(feature = "mobile"))]
        let modal = show_confirm().then(|| {
            rsx! {
                AuthorDeleteModal {
                    author_id: a.id,
                    author_name: a.name.clone(),
                    book_count: a.book_count,
                    server_url: server_url.clone(),
                    state: AuthorDeleteState {
                        show_confirm,
                        deleting,
                        delete_error,
                    },
                }
            }
        });
        #[cfg(feature = "mobile")]
        let modal: Option<Element> = None;
        modal
    };

    let hero_text = AuthorHeroText {
        first,
        last,
        initial: &initial,
        bg_style: &bg_style,
    };
    rsx! {
        div { class: "disc-page", style: "--accent: {accent}",
            {author_hero(&a, &server_url, author, hero_text, admin_actions)}
            {modal}
            {author_body(&series_groups, &standalone)}
        }
    }
}

/// Display strings the hero renders: first/last name split, avatar-letter
/// fallback, and the accent background-gradient CSS. Bundled so
/// `author_hero` stays within clippy's argument-count lint.
struct AuthorHeroText<'a> {
    first: &'a str,
    last: &'a str,
    initial: &'a str,
    bg_style: &'a str,
}

/// Hero header: breadcrumb, avatar (with the hover-revealed photo-edit
/// overlay), name + admin delete affordance, and the book-count stat.
fn author_hero(
    a: &AuthorDetail,
    server_url: &str,
    author: Signal<Option<AuthorDetail>>,
    text: AuthorHeroText<'_>,
    admin_actions: Option<Element>,
) -> Element {
    let AuthorHeroText {
        first,
        last,
        initial,
        bg_style,
    } = text;
    rsx! {
        div { class: "disc-hero", style: "background: {bg_style}",
            nav { class: "breadcrumb",
                Link { to: Route::Landing {}, "Library" }
                span { class: "breadcrumb-sep", " › " }
                span { "{a.name}" }
            }
            div { class: "disc-hero-grid",
                {author_avatar(a, server_url, author, initial)}
                div { class: "disc-hero-info",
                    h1 { class: "disc-hero-title",
                        "{first} "
                        if !last.is_empty() {
                            em { "{last}" }
                        }
                    }
                    {admin_actions}
                }
                div { class: "disc-stat-block",
                    span { class: "disc-stat-label label", "In your library" }
                    span { class: "disc-stat", "{a.book_count}" }
                }
            }
        }
    }
}

/// Avatar — letter fallback by default; F1.11 swaps in the cached profile
/// photo when `has_photo` is set. Wrapped in `AuthorPhotoEditOverlay` so a
/// hover-revealed pencil opens the URL/upload/scan modal. The `on_change`
/// callback re-fetches the author payload so the new photo (or restored
/// letter, after a scan miss) replaces the hero in place.
fn author_avatar(
    a: &AuthorDetail,
    server_url: &str,
    mut author: Signal<Option<AuthorDetail>>,
    initial: &str,
) -> Element {
    // A transient photo-fetch failure otherwise renders the browser's
    // broken-image icon with no self-heal until a full reload. Tracks the
    // *url* that failed (not just a bool) so a mobile token rotation —
    // which changes the `media_url` query string — gets a fresh chance to
    // load. That alone isn't enough for a same-URL content change though:
    // the photo endpoint is keyed only by author id, so a successful
    // re-upload via the edit overlay produces the *same* url. `on_change`
    // fires exactly when that happens, so it explicitly resets too.
    let mut broken_photo_src: Signal<Option<String>> = use_signal(|| None);
    let photo_url = a
        .has_photo
        .then(|| crate::media_url(server_url, &format!("/api/authors/{}/photo", a.id)));
    rsx! {
        AuthorPhotoEditOverlay {
            author_id: a.id,
            author_name: a.name.clone(),
            server_url: server_url.to_string(),
            on_change: {
                let server_url = server_url.to_string();
                let author_id = a.id;
                move |_| {
                    let server_url = server_url.clone();
                    broken_photo_src.set(None);
                    spawn(async move {
                        if let Ok(a2) = data::get_author(&server_url, author_id).await {
                            author.set(a2);
                        }
                    });
                }
            },
            if let Some(url) =
                photo_url.filter(|u| broken_photo_src.read().as_deref() != Some(u.as_str()))
            {
                img {
                    class: "disc-avatar disc-avatar--photo",
                    // `media_url` server-prefixes and (mobile) appends the
                    // session token so the WebView's `<img>` fetch
                    // authenticates; no-op on web.
                    src: "{url}",
                    alt: "{a.name}",
                    onerror: {
                        let url = url.clone();
                        move |_| broken_photo_src.set(Some(url.clone()))
                    },
                }
            } else {
                div { class: "disc-avatar", "{initial}" }
            }
        }
    }
}

/// One (series name, series id, books-in-series) group per distinct
/// series, in first-seen order.
type SeriesGroups<'a> = Vec<(String, i64, Vec<&'a EbookMetadata>)>;

/// Body: books grouped by series (each in its own section, linking to
/// `SeriesDetail` when the series is a real row), then a trailing
/// "Other works" section for standalone titles.
fn author_body(series_groups: &SeriesGroups<'_>, standalone: &[&EbookMetadata]) -> Element {
    rsx! {
        div { class: "disc-body",
            for (series_name, series_id, books) in series_groups.iter() {
                div { class: "disc-section",
                    div { class: "disc-section-head",
                        span { class: "label", "Series" }
                        if *series_id > 0 {
                            Link {
                                to: Route::SeriesDetail { id: *series_id },
                                class: "disc-section-title",
                                h2 { "{series_name}" }
                            }
                        } else {
                            h2 { class: "disc-section-title", "{series_name}" }
                        }
                    }
                    div { class: "disc-grid",
                        for book in books.iter() {
                            Link {
                                key: "{book.id}",
                                to: Route::BookDetail { uuid: book.unique_identifier.clone().unwrap_or_default() },
                                class: "lib-tile",
                                Cover { book: (*book).clone() }
                                div { class: "lib-tile-title",
                                    if let Some(ref idx) = book.series_index {
                                        "#{idx} · "
                                    }
                                    "{book.title.as_deref().unwrap_or(&book.filename)}"
                                }
                            }
                        }
                    }
                }
            }

            if !standalone.is_empty() {
                div { class: "disc-section",
                    div { class: "disc-section-head",
                        span { class: "label", "Other works" }
                        h2 { class: "disc-section-title", "Standalone & novellas" }
                    }
                    div { class: "disc-grid",
                        for book in standalone.iter() {
                            Link {
                                key: "{book.id}",
                                to: Route::BookDetail { uuid: book.unique_identifier.clone().unwrap_or_default() },
                                class: "lib-tile",
                                Cover { book: (*book).clone() }
                                div { class: "lib-tile-title",
                                    "{book.title.as_deref().unwrap_or(&book.filename)}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Splits an author's display name into first-name / rest, for the
/// hero title's `First ` + *Rest* italic styling.
fn split_name(name: &str) -> (&str, &str) {
    let parts: Vec<&str> = name.splitn(2, ' ').collect();
    let first = parts.first().copied().unwrap_or("");
    let last = if parts.len() > 1 { parts[1] } else { "" };
    (first, last)
}

/// Uppercased first character of the name, for the letter-fallback avatar.
fn author_initial(name: &str) -> String {
    name.chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string()
}

/// Groups an author's books by series (in first-seen order, each series'
/// books in scan order), with non-series titles collected separately.
fn group_books_by_series(books: &[EbookMetadata]) -> (SeriesGroups<'_>, Vec<&EbookMetadata>) {
    let mut series_groups: SeriesGroups<'_> = Vec::new();
    let mut standalone: Vec<&EbookMetadata> = Vec::new();

    for book in books {
        if let Some(ref series_name) = book.series {
            if let Some(group) = series_groups
                .iter_mut()
                .find(|(name, _, _)| name == series_name)
            {
                group.2.push(book);
            } else {
                let sid = book.series_id.unwrap_or(0);
                series_groups.push((series_name.clone(), sid, vec![book]));
            }
        } else {
            standalone.push(book);
        }
    }

    (series_groups, standalone)
}

/// Transient state for the delete-author modal: whether the confirm
/// pane is open, whether a delete RPC is in flight, and the most
/// recent error message (if any). Grouped because all three change
/// together across the confirm/run/error lifecycle, and keep the
/// modal's signature focused on the stable identity props.
#[cfg(not(feature = "mobile"))]
#[derive(Clone, Copy, PartialEq)]
struct AuthorDeleteState {
    show_confirm: Signal<bool>,
    deleting: Signal<bool>,
    delete_error: Signal<Option<String>>,
}

/// Confirmation modal for the admin "Delete author" action. On confirm,
/// hits the `rpc_delete_author` server fn (which un-links every book,
/// inserts the name into `ignored_authors`, and refreshes FTS) then
/// navigates back to `/authors`. The blocklist insert is what makes the
/// delete durable across reindexes — without it the next `Task::Scan`
/// would silently recreate the row from the OPF metadata. Web-only;
/// mobile admins fall back to the per-book metadata edit page.
#[cfg(not(feature = "mobile"))]
#[component]
fn AuthorDeleteModal(
    author_id: i64,
    author_name: String,
    book_count: usize,
    server_url: String,
    state: AuthorDeleteState,
) -> Element {
    let AuthorDeleteState {
        show_confirm,
        deleting,
        delete_error,
    } = state;
    let mut show_confirm = show_confirm;
    let mut deleting = deleting;
    let mut delete_error = delete_error;
    let nav = use_navigator();
    let busy = deleting();
    let book_count_label = if book_count == 1 { "book" } else { "books" };

    rsx! {
        div {
            class: "author-delete-modal-backdrop",
            role: "dialog",
            aria_modal: "true",
            aria_label: "Delete {author_name}",
            "data-testid": "author-delete-modal",
            onclick: move |_| {
                if !busy {
                    show_confirm.set(false);
                }
            },
            div {
                class: "author-delete-modal",
                onclick: move |evt| evt.stop_propagation(),
                h2 { class: "author-delete-modal__title", "Delete \"{author_name}\"?" }
                p { class: "author-delete-modal__body",
                    "This will un-link the author from {book_count} {book_count_label} "
                    "and prevent the name from being re-added on future library scans. "
                    "The books themselves are not deleted."
                }
                if let Some(msg) = delete_error() {
                    p { role: "alert", class: "error author-delete-modal__error", "⚠ {msg}" }
                }
                div { class: "author-delete-modal__actions",
                    button {
                        class: "btn",
                        "data-testid": "author-delete-cancel",
                        disabled: busy,
                        onclick: move |_| {
                            show_confirm.set(false);
                        },
                        "Cancel"
                    }
                    button {
                        class: "btn primary author-delete-confirm",
                        "data-testid": "author-delete-confirm",
                        disabled: busy,
                        onclick: {
                            let server_url = server_url.clone();
                            move |_| {
                                let server_url = server_url.clone();
                                let nav = nav;
                                spawn(async move {
                                    deleting.set(true);
                                    delete_error.set(None);
                                    match data::delete_author(&server_url, author_id).await {
                                        Ok(_) => {
                                            show_confirm.set(false);
                                            nav.push(Route::AuthorsIndex {});
                                        }
                                        Err(e) => {
                                            delete_error.set(Some(e.to_string()));
                                        }
                                    }
                                    deleting.set(false);
                                });
                            }
                        },
                        if busy { "Deleting\u{2026}" } else { "Delete" }
                    }
                }
            }
        }
    }
}
