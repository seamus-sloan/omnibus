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

/// Route target for `/read/:uuid` — the F2.2 immersive EPUB reader.
/// Deliberately rendered **without** [`ScreenLayout`]: the reader is a
/// full-screen surface with its own slim control bar, so the app's top/bottom
/// nav is suppressed. Same uuid-keyed stability rationale as [`BookDetail`].
#[component]
pub fn BookRead(uuid: String) -> Element {
    rsx! {
        BookReadPage { uuid }
    }
}

/// Route target for `/listen/:uuid` — the F2.3 immersive audiobook
/// player. Same uuid-keyed stability + no-chrome rationale as
/// [`BookRead`]; the player owns its own slim top bar.
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

/// Route target for `/authors` — browse-all authors index (F1.12).
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

/// Route target for `/series` — browse-all series index (F1.12).
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
