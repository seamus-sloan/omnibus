//! Top-level [`Route`] enum and per-route path bindings.
//!
//! Single source of truth for navigation across every frontend target;
//! each variant maps a URL pattern to a page in [`crate::pages`]. Wrapped
//! by [`crate::ScreenLayout`] so the nav chrome stays consistent.

use dioxus::prelude::*;
use dioxus_router::Routable;

use crate::pages::*;
use crate::{use_page_title, ScreenLayout};

/// Top-level router for every omnibus frontend target.
#[derive(Clone, Debug, PartialEq, Eq, Routable)]
pub enum Route {
    #[route("/")]
    Landing {},
    #[route("/settings")]
    Settings {},
    #[route("/account")]
    Account {},
    #[route("/add-books")]
    AddBooks {},
    #[route("/books/:uuid")]
    BookDetail { uuid: String },
    #[route("/books/:uuid/edit")]
    MetadataEdit { uuid: String },
    #[route("/read/:uuid")]
    BookRead { uuid: String },
    #[route("/listen/:uuid")]
    BookListen { uuid: String },
    #[route("/authors")]
    AuthorsIndex {},
    #[route("/authors/:id")]
    AuthorDetail { id: i64 },
    #[route("/series")]
    SeriesIndex {},
    #[route("/series/:id")]
    SeriesDetail { id: i64 },
    #[route("/tags")]
    TagCloud {},
    #[route("/shelves")]
    Shelves {},
    #[route("/shelves/:id")]
    ShelfDetail { id: i64 },
    #[route("/search")]
    MobileSearch {},
    #[route("/search/:query")]
    Search { query: String },
    #[route("/connect")]
    ServerConnect {},
    #[route("/login")]
    Login {},
    #[route("/register")]
    Register {},
}

/// Route target for `/` — wraps [`LandingPage`] in the platform screen layout.
#[component]
pub fn Landing() -> Element {
    use_page_title(|| None);
    rsx! {
        ScreenLayout { LandingPage {} }
    }
}

/// Route target for `/settings` — wraps [`SettingsPage`] in the platform screen layout.
#[component]
pub fn Settings() -> Element {
    use_page_title(|| Some("Settings".into()));
    rsx! {
        ScreenLayout { SettingsPage {} }
    }
}

/// Route target for `/account` — wraps [`AccountPage`] in the platform screen
/// layout. Web renders the Send-to-Kindle destination form; the native shell
/// renders the mobile "You" tab.
#[component]
pub fn Account() -> Element {
    use_page_title(|| Some("Account".into()));
    rsx! {
        ScreenLayout { AccountPage {} }
    }
}

/// Route target for `/add-books` — wraps [`AddBooksPage`] in the platform screen layout.
#[component]
pub fn AddBooks() -> Element {
    use_page_title(|| Some("Add Books".into()));
    rsx! {
        ScreenLayout { AddBooksPage {} }
    }
}

/// Route target for `/books/:uuid` — the detail page is uuid-keyed so
/// bookmarked URLs survive reindexes (`books.id` is `AUTOINCREMENT` and
/// the indexer's DELETE+INSERT path renumbers it on every run;
/// `books.uuid` is a deterministic UUIDv5 of `(library_path, filename)`
/// that stays stable across reindexes and re-installs).
#[component]
pub fn BookDetail(uuid: String) -> Element {
    rsx! {
        ScreenLayout { BookDetailPage { uuid } }
    }
}

/// Route target for `/books/:uuid/edit` — metadata edit form. Same
/// stability rationale as [`BookDetail`].
#[component]
pub fn MetadataEdit(uuid: String) -> Element {
    use_page_title(|| Some("Edit Metadata".into()));
    rsx! {
        ScreenLayout { MetadataEditPage { uuid } }
    }
}

/// Route target for `/read/:uuid` — the immersive EPUB reader.
/// Deliberately rendered **without** [`ScreenLayout`]: the reader is a
/// full-screen surface with its own slim control bar, so the app's top/bottom
/// nav is suppressed. Same uuid-keyed stability rationale as [`BookDetail`].
#[component]
pub fn BookRead(uuid: String) -> Element {
    use_page_title(|| Some("Reader".into()));
    rsx! {
        BookReadPage { uuid }
    }
}

/// Route target for `/listen/:uuid` — the immersive audiobook player.
/// Same uuid-keyed stability + no-chrome rationale as [`BookRead`]; the
/// player owns its own slim top bar.
#[component]
pub fn BookListen(uuid: String) -> Element {
    use_page_title(|| Some("Player".into()));
    rsx! {
        BookListenPage { uuid }
    }
}

/// Route target for `/connect` — the mobile pre-login server-URL entry
/// screen. Rendered without [`ScreenLayout`] (like [`Login`]) so it's
/// reachable before authentication; on web it redirects to `/`.
#[component]
pub fn ServerConnect() -> Element {
    use_page_title(|| Some("Connect".into()));
    rsx! { ServerConnectPage {} }
}

/// Route target for `/login` — credential entry form. Rendered without the
/// main screen chrome so the login flow stands alone. `LoginPage` owns its
/// own full-page chrome via [`crate::components::auth::AuthShell`].
#[component]
pub fn Login() -> Element {
    use_page_title(|| Some("Log in".into()));
    rsx! { LoginPage {} }
}

/// Route target for `/authors/:id` — single author discovery page.
#[component]
pub fn AuthorDetail(id: i64) -> Element {
    rsx! {
        ScreenLayout { AuthorPage { id } }
    }
}

/// Route target for `/authors` — browse-all authors index.
#[component]
pub fn AuthorsIndex() -> Element {
    use_page_title(|| Some("Authors".into()));
    rsx! {
        ScreenLayout { AuthorsIndexPage {} }
    }
}

/// Route target for `/series/:id` — single series discovery page.
#[component]
pub fn SeriesDetail(id: i64) -> Element {
    rsx! {
        ScreenLayout { SeriesPage { id } }
    }
}

/// Route target for `/series` — browse-all series index.
#[component]
pub fn SeriesIndex() -> Element {
    use_page_title(|| Some("Series".into()));
    rsx! {
        ScreenLayout { SeriesIndexPage {} }
    }
}

/// Route target for `/tags` — tag cloud discovery page.
#[component]
pub fn TagCloud() -> Element {
    use_page_title(|| Some("Tags".into()));
    rsx! {
        ScreenLayout { TagCloudPage {} }
    }
}

/// Route target for `/shelves` — the shelves index (mobile-first; web renders a plain list).
#[component]
pub fn Shelves() -> Element {
    use_page_title(|| Some("Shelves".into()));
    rsx! {
        ScreenLayout { ShelvesIndexPage {} }
    }
}

/// Route target for `/shelves/:id` — one shelf's detail surface.
#[component]
pub fn ShelfDetail(id: i64) -> Element {
    rsx! {
        ScreenLayout { ShelfDetailPage { id } }
    }
}

/// Route target for `/search` — the mobile-native live search screen.
///
/// Mobile wraps [`MobileSearchPage`] in [`ScreenLayout`] so the bottom nav is
/// retained (matching the `Search & discovery` design). On web `/search` has no
/// query — web search is the ⌘K palette — so it redirects to the landing page.
#[cfg(feature = "mobile")]
#[component]
pub fn MobileSearch() -> Element {
    use_page_title(|| Some("Search".into()));
    rsx! {
        ScreenLayout { MobileSearchPage {} }
    }
}

/// Non-mobile stub for `/search`: redirect to the landing page. Web reaches
/// full results via the ⌘K palette's `/search/:query`, never a bare `/search`.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn MobileSearch() -> Element {
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Landing {});
    });
    rsx! {}
}

/// Route target for `/search/:query` — full-page search results.
#[component]
pub fn Search(query: String) -> Element {
    let heading = query.clone();
    use_page_title(move || Some(format!("Search: {heading}")));
    rsx! {
        ScreenLayout { SearchPage { query } }
    }
}

/// Route target for `/register` — account-creation form. Same chrome as
/// [`Login`] so the two pages feel like one flow.
#[component]
pub fn Register() -> Element {
    use_page_title(|| Some("Register".into()));
    rsx! { RegisterPage {} }
}
