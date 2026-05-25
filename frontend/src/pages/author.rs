//! Author discovery page — shows author details and their books grouped by
//! series, mirroring the `AuthorScreen` design comp from
//! `screens/discovery.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::AuthorDetail;

use crate::components::atrium::Cover;
use crate::components::author_photo_edit::AuthorPhotoEditOverlay;
use crate::{data, use_server_url, Route};

#[component]
pub fn AuthorPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut author: Signal<Option<AuthorDetail>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

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
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    }
    let Some(a) = author() else {
        return rsx! {
            p { class: "subtitle", "Author not found." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    };

    render_author(a, server_url, author)
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

fn render_author(
    a: AuthorDetail,
    server_url: String,
    mut author: Signal<Option<AuthorDetail>>,
) -> Element {
    // Derive accent from the first book that has one, or fall back to theme.
    let accent = a
        .books
        .iter()
        .find_map(|b| b.accent.as_deref())
        .unwrap_or("var(--accent)");

    // Split first / last name for italic styling.
    let parts: Vec<&str> = a.name.splitn(2, ' ').collect();
    let first = parts.first().copied().unwrap_or("");
    let last = if parts.len() > 1 { parts[1] } else { "" };

    // Initial letter for the avatar.
    let initial = a
        .name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    // Group books by series — series books first, standalone after.
    let mut series_groups: Vec<(String, i64, Vec<&omnibus_shared::EbookMetadata>)> = Vec::new();
    let mut standalone: Vec<&omnibus_shared::EbookMetadata> = Vec::new();

    for book in &a.books {
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

    let bg_style = format!(
        "radial-gradient(50% 80% at 80% 20%, color-mix(in oklch, {accent} 14%, transparent), transparent 70%)"
    );

    rsx! {
        div { class: "disc-page", style: "--accent: {accent}",
            // Hero header
            div { class: "disc-hero", style: "background: {bg_style}",
                // Breadcrumb
                nav { class: "breadcrumb",
                    Link { to: Route::Landing {}, "Library" }
                    span { " › " }
                    span { "{a.name}" }
                }
                div { class: "disc-hero-grid",
                    // Avatar — letter fallback by default; F1.11 swaps in
                    // the cached profile photo when `has_photo` is set.
                    // Wrapped in `AuthorPhotoEditOverlay` so a hover-revealed
                    // pencil opens the URL/upload/scan modal. The on_change
                    // callback re-fetches the author payload so the new
                    // photo (or restored letter, after a scan miss) replaces
                    // the hero in place.
                    AuthorPhotoEditOverlay {
                        author_id: a.id,
                        author_name: a.name.clone(),
                        server_url: server_url.clone(),
                        on_change: {
                            let server_url = server_url.clone();
                            let author_id = a.id;
                            move |_| {
                                let server_url = server_url.clone();
                                spawn(async move {
                                    if let Ok(a2) =
                                        data::get_author(&server_url, author_id).await
                                    {
                                        author.set(a2);
                                    }
                                });
                            }
                        },
                        if a.has_photo {
                            img {
                                class: "disc-avatar disc-avatar--photo",
                                // Prefix with `server_url` so mobile WebViews
                                // hit the configured backend origin instead of
                                // the in-app document origin. `server_url` is
                                // empty on web, so this is a no-op there.
                                src: "{server_url}/api/authors/{a.id}/photo",
                                alt: "{a.name}",
                            }
                        } else {
                            div { class: "disc-avatar", "{initial}" }
                        }
                    }
                    // Name + metadata
                    div { class: "disc-hero-info",
                        h1 { class: "disc-hero-title",
                            "{first} "
                            if !last.is_empty() {
                                em { "{last}" }
                            }
                        }
                    }
                    // Book count stat
                    div { class: "disc-stat-block",
                        span { class: "disc-stat-label label", "In your library" }
                        span { class: "disc-stat", "{a.book_count}" }
                    }
                }
            }

            // Body: books grouped by series
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
}
