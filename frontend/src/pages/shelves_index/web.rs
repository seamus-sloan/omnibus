//! Web shelves index (`/shelves`): every shelf the reader can see, grouped by
//! owner with the reader's own first, narrowed by a text filter over shelf and
//! owner names plus owner and kind toggles, in a persisted sort. "New shelf"
//! lands on the shelf it made.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::{IndexSort, Shelf};

use super::card::ShelfCard;
use super::filter::{
    census, empty_message, group_shelves, group_title, owners, shows_others_private, KindFilter,
    OwnerFilter, OwnerGroup, ShelfOwner, ShelfQuery,
};
use crate::components::shelf_glyphs::lock_icon;
use crate::components::user_avatar::UserAvatar;
use crate::components::{CreateShelfModal, PageLoading};
use crate::pages::index_shell::{
    index_page_early_return, use_index_page_shell, IndexFilterInput, IndexPageState,
    IndexSortToggle,
};
use crate::{data, index_prefs, use_server_url, AvatarCacheBust, Route};

/// The web shelves index — see the module doc.
#[component]
pub(super) fn WebShelvesIndex() -> Element {
    let server_url = use_server_url();
    let fetch_url = server_url.clone();
    let shell = use_index_page_shell(
        move || {
            let url = fetch_url.clone();
            async move { data::list_shelves(&url).await }
        },
        || index_prefs::load().shelves_sort,
    );
    let IndexPageState {
        items: shelves,
        loading,
        error,
        mut filter,
        mut sort,
    } = shell;
    let mut owner = use_signal(OwnerFilter::default);
    let mut kind = use_signal(KindFilter::default);
    let mut show_create = use_signal(|| false);
    let nav = use_navigator();
    // `None` until the boot probe answers. The list waits for it, so the
    // reader's own group is never regrouped under their eyes when it lands.
    let viewer_probe = crate::use_current_user().0;
    let avatar_bust = try_use_context::<AvatarCacheBust>().map_or(0, |b| (b.0)());

    let groups = use_memo(move || {
        let viewer_id = viewer_probe().flatten().map(|u| u.id);
        let query = ShelfQuery {
            text: filter(),
            owner: owner(),
            kind: kind(),
            sort: sort(),
        };
        group_shelves(&shelves.read(), viewer_id, &query)
    });
    let owner_list = use_memo(move || {
        let viewer_id = viewer_probe().flatten().map(|u| u.id);
        owners(&shelves.read(), viewer_id)
    });

    if let Some(early) = index_page_early_return(loading, error) {
        return early;
    }
    let Some(viewer) = viewer_probe() else {
        return rsx! { PageLoading {} };
    };

    let viewer_id = viewer.as_ref().map(|u| u.id);
    let query = ShelfQuery {
        text: filter(),
        owner: owner(),
        kind: kind(),
        sort: sort(),
    };
    let all = shelves.read();
    let header = HeaderView {
        census: census(&all, viewer_id),
        admin_note: viewer.as_ref().is_some_and(|u| u.is_admin)
            && shows_others_private(&all, viewer_id),
    };
    let total = all.len();
    drop(all);
    let owner_list = owner_list();
    let empty = EmptyView {
        message: empty_message(&query, total > 0),
        filtering: query.is_filtering(),
    };
    let actions = BodyActions {
        on_clear: EventHandler::new(move |()| {
            filter.set(String::new());
            owner.set(OwnerFilter::Anyone);
            kind.set(KindFilter::Any);
        }),
        on_new: EventHandler::new(move |()| show_create.set(true)),
    };

    rsx! {
        div { class: "idx-page shv-page", "data-testid": "shelves-index",
            ShelvesIndexHeader { view: header, on_new: actions.on_new }
            ShelvesToolbar {
                text: query.text.clone(),
                kind: query.kind,
                sort: query.sort,
                on_text: move |v| filter.set(v),
                on_kind: move |k| kind.set(k),
                on_sort: move |s: IndexSort| {
                    sort.set(s);
                    let mut prefs = index_prefs::load();
                    prefs.shelves_sort = s;
                    index_prefs::save(&prefs);
                },
            }
            if owner_list.len() > 1 {
                OwnerChips {
                    owners: owner_list,
                    selected: query.owner,
                    total,
                    avatar_bust,
                    on_select: move |o| owner.set(o),
                }
            }
            {shelves_body(&groups.read(), empty, &server_url, avatar_bust, actions)}
        }
        if show_create() {
            CreateShelfModal {
                on_close: move |_| show_create.set(false),
                on_created: move |created: Shelf| {
                    show_create.set(false);
                    nav.push(Route::ShelfDetail { id: created.id });
                },
            }
        }
    }
}

/// Header fields derived from the loaded list.
#[derive(Clone, PartialEq)]
struct HeaderView {
    census: String,
    /// Set for an admin whose list includes other readers' private shelves.
    admin_note: bool,
}

/// Hero title, census line, and the primary New shelf action.
#[component]
fn ShelvesIndexHeader(view: HeaderView, on_new: EventHandler<()>) -> Element {
    rsx! {
        div { class: "idx-header shv-header",
            div { class: "shv-head-row",
                div {
                    h1 { class: "disc-hero-title", "Shelves" }
                    p { class: "idx-subtitle", "data-testid": "shelves-census", "{view.census}" }
                    if view.admin_note {
                        p { class: "shv-admin-note", "data-testid": "shelves-admin-note",
                            {lock_icon()}
                            "Includes other readers\u{2019} private shelves \u{2014} you can see them as an admin."
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "btn primary shv-new",
                    "data-testid": "new-shelf",
                    onclick: move |_| on_new.call(()),
                    span { class: "shv-new-plus", aria_hidden: "true", "\u{FF0B}" }
                    "New shelf"
                }
            }
        }
    }
}

/// Text filter, kind toggle, and sort toggle.
#[component]
fn ShelvesToolbar(
    text: String,
    kind: KindFilter,
    sort: IndexSort,
    on_text: EventHandler<String>,
    on_kind: EventHandler<KindFilter>,
    on_sort: EventHandler<IndexSort>,
) -> Element {
    rsx! {
        div { class: "idx-toolbar shv-toolbar",
            IndexFilterInput {
                filter: text,
                placeholder: "Filter by shelf or owner\u{2026}",
                aria_label: "Filter shelves by name or owner",
                testid: "shelves-filter",
                on_filter: on_text,
            }
            div { class: "idx-sort", role: "group", "aria-label": "Shelf kind",
                span { class: "label", "Kind" }
                for k in KindFilter::ALL {
                    button {
                        key: "{k.token()}",
                        r#type: "button",
                        class: "idx-btn",
                        "aria-pressed": if kind == k { "true" } else { "false" },
                        "data-testid": "shelves-kind-{k.token()}",
                        onclick: move |_| on_kind.call(k),
                        "{k.label()}"
                    }
                }
            }
            IndexSortToggle {
                sort,
                name_label: "A\u{2013}Z",
                testid_prefix: "shelves",
                on_sort,
            }
        }
    }
}

/// "Everyone" plus one chip per owner. Picking a chip narrows the list to that
/// reader's shelves; picking it again widens back to everyone.
#[component]
fn OwnerChips(
    owners: Vec<ShelfOwner>,
    selected: OwnerFilter,
    total: usize,
    avatar_bust: u32,
    on_select: EventHandler<OwnerFilter>,
) -> Element {
    let everyone = selected == OwnerFilter::Anyone;
    rsx! {
        div { class: "shv-owners", role: "group", "aria-label": "Owner",
            span { class: "label", "Owner" }
            button {
                r#type: "button",
                class: if everyone { "chip on shv-owner-chip shv-owner-chip--all" } else { "chip shv-owner-chip shv-owner-chip--all" },
                "aria-pressed": if everyone { "true" } else { "false" },
                "data-testid": "shelves-owner-anyone",
                onclick: move |_| on_select.call(OwnerFilter::Anyone),
                "Everyone"
                span { class: "count", "{total}" }
            }
            for o in owners.iter() {
                {owner_chip(o, selected, avatar_bust, on_select)}
            }
        }
    }
}

/// One owner's chip: avatar, "You" or their name, and their shelf count.
fn owner_chip(
    owner: &ShelfOwner,
    selected: OwnerFilter,
    avatar_bust: u32,
    on_select: EventHandler<OwnerFilter>,
) -> Element {
    let pick = OwnerFilter::Owner(owner.id);
    let on = selected == pick;
    let next = if on { OwnerFilter::Anyone } else { pick };
    let label = if owner.is_viewer {
        "You".to_string()
    } else {
        owner.name.clone()
    };
    rsx! {
        button {
            key: "{owner.id}",
            r#type: "button",
            class: if on { "chip on shv-owner-chip" } else { "chip shv-owner-chip" },
            "aria-pressed": if on { "true" } else { "false" },
            "data-testid": "shelves-owner-{owner.id}",
            onclick: move |_| on_select.call(next),
            span { class: "shv-chip-av", aria_hidden: "true",
                UserAvatar {
                    user_id: owner.id,
                    name: owner.name.clone(),
                    has_avatar: owner.has_avatar,
                    class: "shv-av",
                    bust: avatar_bust,
                }
            }
            "{label}"
            span { class: "count", "{owner.shelf_count}" }
        }
    }
}

/// What the body says when no card survives the filters.
#[derive(Clone, PartialEq)]
struct EmptyView {
    message: String,
    /// A filter hid everything, so the way out is clearing it, not a new shelf.
    filtering: bool,
}

/// The body's two actions, bundled so [`shelves_body`] stays under clippy's
/// argument cap.
#[derive(Clone, Copy)]
struct BodyActions {
    on_clear: EventHandler<()>,
    on_new: EventHandler<()>,
}

/// The owner groups, or the empty state when nothing survives the filters.
fn shelves_body(
    groups: &[OwnerGroup],
    empty: EmptyView,
    server_url: &str,
    avatar_bust: u32,
    actions: BodyActions,
) -> Element {
    if groups.is_empty() {
        return rsx! {
            div { class: "shv-empty", "data-testid": "shelves-empty",
                p { class: "shv-empty-msg", "{empty.message}" }
                if empty.filtering {
                    button {
                        r#type: "button",
                        class: "btn",
                        "data-testid": "shelves-clear-filters",
                        onclick: move |_| actions.on_clear.call(()),
                        "Clear filters"
                    }
                } else {
                    button {
                        r#type: "button",
                        class: "btn primary",
                        "data-testid": "shelves-empty-new",
                        onclick: move |_| actions.on_new.call(()),
                        "\u{FF0B} New shelf"
                    }
                }
            }
        };
    }
    rsx! {
        div { class: "shv-body",
            for group in groups.iter() {
                {owner_group(group, server_url, avatar_bust)}
            }
        }
    }
}

/// One owner's section: avatar, "Your shelves" / "<name>'s shelves", count,
/// then their cards.
fn owner_group(group: &OwnerGroup, server_url: &str, avatar_bust: u32) -> Element {
    let owner = &group.owner;
    let heading_id = format!("shv-group-{}", owner.id);
    let count = group.shelves.len();
    rsx! {
        section {
            key: "{owner.id}",
            class: "shv-group",
            "data-testid": "shelves-group-{owner.id}",
            "aria-labelledby": "{heading_id}",
            header { class: "shv-group-head",
                span { class: "shv-group-av", aria_hidden: "true",
                    UserAvatar {
                        user_id: owner.id,
                        name: owner.name.clone(),
                        has_avatar: owner.has_avatar,
                        class: "shv-av",
                        bust: avatar_bust,
                    }
                }
                h2 { class: "shv-group-title", id: "{heading_id}", "{group_title(owner)}" }
                span { class: "mono shv-group-count", "{count}" }
            }
            div { class: "shv-grid", role: "list",
                for shelf in group.shelves.iter() {
                    ShelfCard {
                        key: "{shelf.id}",
                        shelf: shelf.clone(),
                        is_viewer: owner.is_viewer,
                        server_url: server_url.to_string(),
                        avatar_bust,
                    }
                }
            }
        }
    }
}
