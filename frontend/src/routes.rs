//! Top-level [`Route`] enum and per-route path bindings.
//!
//! Single source of truth for navigation across every frontend target;
//! each variant maps a URL pattern to a page in [`crate::pages`]. Wrapped
//! by [`crate::ScreenLayout`] so the nav chrome stays consistent.

use dioxus::prelude::*;
use dioxus_router::Routable;

use crate::pages::*;
use crate::ScreenLayout;

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
    #[route("/search/:query")]
    Search { query: String },
    #[route("/login")]
    Login {},
    #[route("/register")]
    Register {},
}

/// Route target for `/` — wraps [`LandingPage`] in the platform screen layout.
#[component]
pub fn Landing() -> Element {
    rsx! {
        ScreenLayout { LandingPage {} }
    }
}

/// Route target for `/settings` — wraps [`SettingsPage`] in the platform screen layout.
#[component]
pub fn Settings() -> Element {
    rsx! {
        ScreenLayout { SettingsPage {} }
    }
}

/// Route target for `/account` — wraps [`AccountPage`] in the platform screen
/// layout. Web renders the Send-to-Kindle destination form; the native shell
/// renders the mobile "You" tab.
#[component]
pub fn Account() -> Element {
    rsx! {
        ScreenLayout { AccountPage {} }
    }
}

/// Route target for `/add-books` — wraps [`AddBooksPage`] in the platform screen layout.
#[component]
pub fn AddBooks() -> Element {
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
    rsx! {
        BookReadPage { uuid }
    }
}

/// Route target for `/listen/:uuid` — the immersive audiobook player.
/// Same uuid-keyed stability + no-chrome rationale as [`BookRead`]; the
/// player owns its own slim top bar.
#[component]
pub fn BookListen(uuid: String) -> Element {
    rsx! {
        BookListenPage { uuid }
    }
}

/// Route target for `/login` — credential entry form. Rendered without the
/// main screen chrome so the login flow stands alone. `LoginPage` owns its
/// own full-page chrome via [`crate::components::auth::AuthShell`].
#[component]
pub fn Login() -> Element {
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
    rsx! {
        ScreenLayout { SeriesIndexPage {} }
    }
}

/// Route target for `/tags` — tag cloud discovery page.
#[component]
pub fn TagCloud() -> Element {
    rsx! {
        ScreenLayout { TagCloudPage {} }
    }
}

/// Route target for `/shelves` — the shelves index (mobile-first; web renders a plain list).
#[component]
pub fn Shelves() -> Element {
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

/// Route target for `/search/:query` — full-page search results.
#[component]
pub fn Search(query: String) -> Element {
    rsx! {
        ScreenLayout { SearchPage { query } }
    }
}

/// Route target for `/register` — account-creation form. Same chrome as
/// [`Login`] so the two pages feel like one flow.
#[component]
pub fn Register() -> Element {
    rsx! { RegisterPage {} }
}
