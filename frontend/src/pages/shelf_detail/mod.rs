//! Shelf detail page (`/shelves/:id`).
//!
//! Renders the shelf rail alongside a main column showing one shelf's header
//! (kind + visibility badges, actions) and its member books. Smart shelves
//! show their rule as chips and an auto-sorted grid; manual shelves show a
//! position-ordered grid with an "Add books" affordance.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, RuleField, RuleOp, Shelf, ShelfRule, SortDir, SortKey};

use crate::components::EditShelfRulesModal;
#[cfg(not(feature = "mobile"))]
use crate::components::{RailActive, ShelvesRail};
use crate::{data, use_server_url, Route};

mod add_books_modal;
#[cfg(not(feature = "mobile"))]
mod body;
#[cfg(not(feature = "mobile"))]
mod header;
#[cfg(feature = "mobile")]
mod mobile;

use add_books_modal::AddBooksModal;
#[cfg(not(feature = "mobile"))]
use body::{web_shelf_body, ShelfBodySignals};

/// Shelf detail page — see the module doc for the smart/manual split.
#[component]
pub fn ShelfDetailPage(id: i64) -> Element {
    let server_url = use_server_url();
    let shelf = use_signal(|| None::<Shelf>);
    let books = use_signal(Vec::<EbookMetadata>::new);
    let loading = use_signal(|| true);
    let error = use_signal(|| None::<String>);
    crate::use_page_title(move || shelf.read().as_ref().map(|s| s.name.clone()));
    let sort_key = use_signal(|| SortKey::Title);
    // Only mobile mutates these directly, so only it needs `mut` bindings.
    #[cfg(feature = "mobile")]
    let mut show_add = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let show_add = use_signal(|| false);
    #[cfg(feature = "mobile")]
    let mut edit_rules = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let edit_rules = use_signal(|| false);
    // Bumped to force a refetch after a membership edit.
    #[cfg(feature = "mobile")]
    let mut reload = use_signal(|| 0u32);
    #[cfg(not(feature = "mobile"))]
    let reload = use_signal(|| 0u32);
    // Set when the member-books refetch fails, so a transient network error
    // renders distinctly from a shelf that is genuinely empty (mirrors
    // `search_mobile.rs`'s `errored` signal for the same failure class).
    let errored = use_signal(|| false);

    use_shelf_effects(
        id,
        server_url.clone(),
        sort_key,
        reload,
        ShelfFetchSignals {
            shelf,
            loading,
            error,
            books,
            errored,
        },
    );

    if loading() && shelf.read().is_none() {
        return render_page_state(id, rsx! { p { class: "subtitle", "Loading\u{2026}" } });
    }

    let Some(current) = shelf.read().clone() else {
        return render_page_state(
            id,
            rsx! {
                p { role: "alert", class: "subtitle",
                    {error().unwrap_or_else(|| "Shelf not found.".into())}
                }
                Link { to: Route::Landing {}, class: "btn", "Back to All books" }
            },
        );
    };

    // Web keeps the rail + toolbar layout; mobile renders the home-style
    // full-screen surface. Both consume the shared fetch pipeline above.
    // (Mobile is a separate build — rule 07 hydration parity is unaffected.)
    #[cfg(feature = "mobile")]
    let body = rsx! {
        mobile::MobileShelfDetail {
            shelf: current.clone(),
            books: books.read().clone(),
            errored: errored(),
            server_url: server_url.clone(),
            on_add: move |_| show_add.set(true),
            on_edit_rules: move |_| edit_rules.set(true),
            on_changed: move |_| reload.with_mut(|n| *n += 1),
        }
    };

    #[cfg(not(feature = "mobile"))]
    let body = web_shelf_body(
        &current,
        &books.read(),
        errored(),
        &server_url,
        ShelfBodySignals {
            sort_key,
            show_add,
            edit_rules,
            reload,
        },
    );

    rsx! {
        {body}
        {shelf_detail_modals(id, current, show_add, edit_rules, reload)}
    }
}

/// The "Add books" and "Edit rules" modals, shown when their respective
/// signals flip true; both bump `reload` on success so the parent refetches.
fn shelf_detail_modals(
    shelf_id: i64,
    current: Shelf,
    mut show_add: Signal<bool>,
    mut edit_rules: Signal<bool>,
    mut reload: Signal<u32>,
) -> Element {
    rsx! {
        if show_add() {
            AddBooksModal {
                shelf_id,
                on_close: move |_| show_add.set(false),
                on_added: move |_| {
                    show_add.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }

        if edit_rules() {
            EditShelfRulesModal {
                shelf: current.clone(),
                on_close: move |_| edit_rules.set(false),
                on_saved: move |_| {
                    edit_rules.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }
    }
}

/// Data signals populated by the shelf detail fetch effects. `Copy` (Dioxus
/// signals), so grouping them keeps [`use_shelf_effects`] under the
/// too-many-arguments cap without changing call-site ergonomics.
#[derive(Clone, Copy)]
struct ShelfFetchSignals {
    shelf: Signal<Option<Shelf>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    books: Signal<Vec<EbookMetadata>>,
    errored: Signal<bool>,
}

/// Wires the two data-fetch effects backing [`ShelfDetailPage`]: the shelf
/// detail itself (id-driven) and its member books (id/sort/reload-driven).
/// Extracted to keep the page component under the line cap; mirrors
/// `book_detail::use_book_data_effects`.
fn use_shelf_effects(
    id: i64,
    server_url: String,
    sort_key: Signal<SortKey>,
    reload: Signal<u32>,
    sig: ShelfFetchSignals,
) {
    let ShelfFetchSignals {
        mut shelf,
        mut loading,
        mut error,
        mut books,
        mut errored,
    } = sig;

    // Fetch the shelf detail whenever the id changes. `id` is a plain prop
    // (not a signal), so it must be wrapped in `use_reactive!` to re-arm this
    // effect on navigation between shelves — see `BookDetailPage` for why.
    let shelf_url = server_url.clone();
    let generation = crate::use_cache_generation();
    use_effect(use_reactive!(|id| {
        let url = shelf_url.clone();
        let _ = reload();
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        spawn(async move {
            let same_shelf = shelf.peek().as_ref().map(|s| s.id) == Some(id);
            if !same_shelf {
                loading.set(true);
            }
            match data::get_shelf(&url, id).await {
                Ok(s) => {
                    shelf.set(Some(s));
                    error.set(None);
                }
                Err(e) => {
                    shelf.set(None);
                    error.set(Some(e.to_string()));
                }
            }
            loading.set(false);
        });
    }));

    // Fetch the member books, re-running on id/sort change or a membership
    // edit. `sort_key()` is a signal read, already tracked; `id` needs the
    // same `use_reactive!` wrapping as above.
    let page_url = server_url;
    use_effect(use_reactive!(|id| {
        let url = page_url.clone();
        let key = sort_key();
        let _ = reload();
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        spawn(async move {
            let dir = default_dir_for(key);
            match data::shelf_page(&url, id, key, dir).await {
                Ok(page) => {
                    books.set(page.books);
                    errored.set(false);
                }
                Err(_) => {
                    // `tracing` isn't linked under the `web` (WASM) feature,
                    // so the signal alone carries the failure to the UI.
                    // Clear the stale list too — otherwise a refetch failure
                    // after navigating shelves leaves the prior shelf's
                    // books on screen under the error banner.
                    books.set(Vec::new());
                    errored.set(true);
                }
            }
        });
    }));
}

/// Loading / not-found chrome. Web wraps in the rail layout; mobile renders
/// the bare screen surface.
fn render_page_state(id: i64, inner: Element) -> Element {
    #[cfg(not(feature = "mobile"))]
    {
        rsx! {
            div { class: "shelf-layout",
                ShelvesRail { active: RailActive::Shelf(id) }
                div { class: "shelf-main", {inner} }
            }
        }
    }
    #[cfg(feature = "mobile")]
    {
        let _ = id;
        rsx! {
            div { class: "m-shelves", {inner} }
        }
    }
}

/// The default sort direction the grid uses for a given sort axis: newest-first
/// for the date axes, ascending otherwise.
fn default_dir_for(key: SortKey) -> SortDir {
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

/// Human-readable summary of one smart rule, e.g. "Tag is Fantasy".
fn rule_text(rule: &ShelfRule) -> String {
    format!(
        "{} {} {}",
        field_label(rule.field),
        op_label(rule.op),
        rule.value
    )
}

/// Display label for a rule field.
fn field_label(field: RuleField) -> &'static str {
    match field {
        RuleField::Tag => "Tag",
        RuleField::Author => "Author",
        RuleField::Series => "Series",
        RuleField::Rating => "Rating",
        RuleField::Status => "Reading status",
        RuleField::Format => "Format",
        RuleField::Year => "Year",
        RuleField::DateAdded => "Date added",
        RuleField::DateUpdated => "Date updated",
    }
}

/// Display label for a rule operator.
fn op_label(op: RuleOp) -> &'static str {
    match op {
        RuleOp::Is => "is",
        RuleOp::IsNot => "is not",
        RuleOp::Contains => "contains",
        RuleOp::StartsWith => "starts with",
        RuleOp::Gte => "is at least",
        RuleOp::Includes => "includes",
        RuleOp::InLast => "in the last",
        RuleOp::Between => "between",
        RuleOp::Before => "before",
        RuleOp::After => "after",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_text_reads_field_op_value() {
        let rule = ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        };
        assert_eq!(rule_text(&rule), "Tag is Fantasy");
    }

    #[test]
    fn default_dir_for_dates_is_desc() {
        assert_eq!(default_dir_for(SortKey::NewestAdded), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::LastUpdated), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::Title), SortDir::Asc);
    }
}
