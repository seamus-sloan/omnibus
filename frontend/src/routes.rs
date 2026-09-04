//! Top-level [`Route`] enum and per-route path bindings.
//!
//! Single source of truth for navigation across every frontend target;
//! each variant maps a URL pattern to a page in [`crate::pages`]. Wrapped
//! by [`crate::ScreenLayout`] so the nav chrome stays consistent.

use dioxus::prelude::*;
use dioxus_router::navigation::NavigationTarget;
use dioxus_router::Routable;

use crate::pages::*;
use crate::{use_page_title, ScreenLayout};

/// Top-level router for every omnibus frontend target.
#[derive(Clone, Debug, PartialEq, Eq, Routable)]
pub enum Route {
    #[route("/")]
    Landing {},
    #[route("/settings?:section")]
    Settings { section: Option<String> },
    #[route("/settings/cleanup/:kind")]
    CleanupReview { kind: String },
    #[route("/logs")]
    Logs {},
    #[route("/admin/health")]
    AdminHealth {},
    #[route("/account")]
    Account {},
    #[route("/add-books")]
    AddBooks {},
    #[route("/check-in")]
    CheckIn {},
    #[route("/books/:uuid")]
    BookDetail { uuid: String },
    #[route("/books/:uuid/edit")]
    MetadataEdit { uuid: String },
    #[route("/read/:uuid")]
    BookRead { uuid: String },
    #[route("/comic/:uuid")]
    ComicRead { uuid: String },
    #[route("/listen/:uuid?:file_id")]
    BookListen { uuid: String, file_id: Option<i64> },
    #[route("/authors")]
    AuthorsIndex {},
    #[route("/authors/:id")]
    AuthorDetail { id: i64 },
    #[route("/series")]
    SeriesIndex {},
    #[route("/series/:id")]
    SeriesDetail { id: i64 },
    #[route("/stats")]
    Stats {},
    #[route("/stats/chart")]
    StatsChart {},
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
    // Must stay last: dioxus-router matches variants in declaration order,
    // so a catch-all above any real route would swallow it.
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

/// Route target for `/` — wraps [`LandingPage`] in the platform screen layout.
#[component]
pub fn Landing() -> Element {
    use_page_title(|| None);
    rsx! {
        ScreenLayout { LandingPage {} }
    }
}

/// Route target for `/settings` — wraps [`SettingsPage`] in the platform screen
/// layout. `section` (the `?section=` query param) selects the active sidebar
/// section on web; the mobile shell ignores it.
#[component]
pub fn Settings(section: Option<String>) -> Element {
    use_page_title(|| Some("Settings".into()));
    rsx! {
        ScreenLayout { SettingsPage { section } }
    }
}

/// Route target for `/settings/cleanup/:kind` — the one-card-at-a-time
/// library-cleanup review queue, wrapped in the platform screen layout.
/// Web/server only; the in-page `use_is_admin` gate (backed by the
/// `AdminUser`-gated `cleanup/*` server functions) keeps the chrome off a
/// non-admin screen.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn CleanupReview(kind: String) -> Element {
    use_page_title(|| Some("Library cleanup".into()));
    rsx! {
        ScreenLayout { CleanupReviewPage { kind } }
    }
}

/// Mobile stub for `/settings/cleanup/:kind`: redirect to the landing page —
/// there is no cleanup review surface on mobile.
#[cfg(feature = "mobile")]
#[component]
pub fn CleanupReview(kind: String) -> Element {
    let _ = kind;
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Landing {});
    });
    rsx! {}
}

/// Route target for `/logs` — the server log viewer now lives inside Settings
/// as the admin-only Logs section, so the standalone route is a redirect that
/// keeps old bookmarks working. Mobile has no log viewer, so it redirects to
/// the landing page instead.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn Logs() -> Element {
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Settings {
            section: Some("logs".into()),
        });
    });
    rsx! {}
}

/// Mobile stub for `/logs`: redirect to the landing page (no log viewer on
/// mobile).
#[cfg(feature = "mobile")]
#[component]
pub fn Logs() -> Element {
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Landing {});
    });
    rsx! {}
}

/// Route target for `/admin/health` — wraps [`AdminHealthPage`] in
/// the platform screen layout. Web/server only; the in-page `use_is_admin`
/// gate (backed by the `AdminUser`-gated `rpc_get_admin_health`) keeps the
/// chrome off a non-admin screen (AC2/AC4).
#[cfg(not(feature = "mobile"))]
#[component]
pub fn AdminHealth() -> Element {
    use_page_title(|| Some("Server health".into()));
    rsx! {
        ScreenLayout { AdminHealthPage {} }
    }
}

/// Mobile stub for `/admin/health`: redirect to the landing page — there is
/// no admin server-health surface on mobile.
#[cfg(feature = "mobile")]
#[component]
pub fn AdminHealth() -> Element {
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Landing {});
    });
    rsx! {}
}

/// Route target for `/account` on web/server — the Account content now lives
/// inside Settings (the same [`AccountPage`], `embedded: true`, behind the
/// sidebar), so this is a redirect that keeps old bookmarks and the bottom
/// nav's "You" tab working, mirroring how `/logs` redirects into Settings
/// above.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn Account() -> Element {
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Settings { section: None });
    });
    rsx! {}
}

/// Route target for `/account` on mobile — wraps [`AccountPage`] in the
/// platform screen layout to render the native "You" tab (identity,
/// now-reading, account rows, theme, sign out). Mobile's Settings page has no
/// Account section (it's a flat library/admin form), so mobile keeps its own
/// route here rather than redirecting.
#[cfg(feature = "mobile")]
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

/// Route target for `/check-in` — wraps [`CheckInPage`] in the platform screen layout.
#[component]
pub fn CheckIn() -> Element {
    use_page_title(|| Some("Check In".into()));
    rsx! {
        ScreenLayout { CheckInPage {} }
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
/// Non-mobile also layers the persistent audiobook [`crate::pages::MiniDock`]
/// alongside the reader — unlike `/listen`, the reader has no
/// transport of its own, so a book playing in the background would
/// otherwise be invisible and uncontrollable while reading.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn BookRead(uuid: String) -> Element {
    use_page_title(|| Some("Reader".into()));
    // `rd-immersive` reflows the reading stage above the docked bar (#1131).
    // Gated on the same predicate as MiniDock's active bar so the reserved
    // space and the visible dock always agree. SSR and the first WASM paint
    // both see an inactive dock (`book`/`uuid` start `None`), so hydration
    // parity holds (rule 07); the class lands post-boot alongside the bar.
    let playback = crate::use_playback();
    let docked = dock_is_active(&playback.book.read(), &playback.uuid.read());
    // `rd-dock-full` marks the web reader host, whose docked bar is the
    // full-width AudioDockBar at the bottom edge (the reader's bottom bar
    // becomes a slim footer above it); mobile keeps the floating mini bar
    // and the base `rd-immersive` geometry.
    rsx! {
        div { class: if docked { "rd-host rd-dock-full rd-immersive" } else { "rd-host rd-dock-full" },
            BookReadPage { uuid }
            MiniDock {}
        }
    }
}

/// Mobile variant of [`BookRead`]: the reader with the persistent
/// [`crate::pages::MobileMiniPlayer`] docked beneath it — launched
/// immersively from book detail, or simply carrying an already-playing
/// audiobook into the reader. Same `rd-immersive` reflow contract as the web
/// variant; mobile renders client-side only, so no hydration constraint.
#[cfg(feature = "mobile")]
#[component]
pub fn BookRead(uuid: String) -> Element {
    use_page_title(|| Some("Reader".into()));
    let ctx = use_context::<MobilePlayback>();
    // Borrowed read (still reactive, unlike `peek`) so the route doesn't
    // clone the whole PlayerView just to derive a boolean each render.
    let docked = mobile_dock_is_active(&ctx.view.read(), (ctx.unsupported)());
    rsx! {
        div { class: if docked { "rd-host rd-immersive" } else { "rd-host" },
            BookReadPage { uuid }
            MobileMiniPlayer {}
        }
    }
}

/// Route target for `/comic/:uuid` — the immersive CBZ comic pager.
/// Same no-chrome + docked-audio contract as [`BookRead`], but a pure
/// `<img>` pager with no epub.js involvement, so it gets its own route
/// instead of threading a format branch through the EPUB reader.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn ComicRead(uuid: String) -> Element {
    use_page_title(|| Some("Comic".into()));
    let playback = crate::use_playback();
    let docked = dock_is_active(&playback.book.read(), &playback.uuid.read());
    rsx! {
        div { class: if docked { "rd-host rd-dock-full rd-immersive" } else { "rd-host rd-dock-full" },
            ComicReadPage { uuid }
            MiniDock {}
        }
    }
}

/// Mobile variant of [`ComicRead`] — same [`crate::pages::MobileMiniPlayer`]
/// docking contract as the mobile [`BookRead`].
#[cfg(feature = "mobile")]
#[component]
pub fn ComicRead(uuid: String) -> Element {
    use_page_title(|| Some("Comic".into()));
    let ctx = use_context::<MobilePlayback>();
    let docked = mobile_dock_is_active(&ctx.view.read(), (ctx.unsupported)());
    rsx! {
        div { class: if docked { "rd-host rd-immersive" } else { "rd-host" },
            ComicReadPage { uuid }
            MobileMiniPlayer {}
        }
    }
}

/// Route target for `/listen/:uuid` — the immersive audiobook player.
/// Same uuid-keyed stability + no-chrome rationale as [`BookRead`]; the
/// player owns its own slim top bar.
#[component]
pub fn BookListen(uuid: String, file_id: Option<i64>) -> Element {
    use_page_title(|| Some("Player".into()));
    rsx! {
        BookListenPage { uuid, file_id }
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

/// Route target for `/stats` — the reading-stats page.
#[component]
pub fn Stats() -> Element {
    use_page_title(|| Some("Stats".into()));
    rsx! {
        ScreenLayout { StatsPage {} }
    }
}

/// Route target for `/stats/chart` — the configurable chart builder.
///
/// A standalone surface while `/stats` is being redesigned, rather than a card
/// on it. Web/server only; mobile has no builder, so it redirects like every
/// other web-only route here.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn StatsChart() -> Element {
    rsx! {
        ScreenLayout { ChartBuilderPage {} }
    }
}

/// Mobile stub for `/stats/chart`: redirect to the landing page.
#[cfg(feature = "mobile")]
#[component]
pub fn StatsChart() -> Element {
    let nav = dioxus_router::use_navigator();
    use_effect(move || {
        nav.replace(Route::Landing {});
    });
    rsx! {}
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

/// Route target for any unmatched URL. Without it dioxus-router renders its
/// own "Failed to parse route" diagnostic — the complete internal route
/// table, admin paths included — as the page body, with no nav to escape by.
#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    use_page_title(|| Some("Not found".into()));
    rsx! {
        ScreenLayout { NotFoundPage { segments } }
    }
}

/// A navigation target for `route` with the dangling `?` trimmed off.
///
/// dioxus-router's macro writes the query separator unconditionally, so a
/// route with an *absent* optional query argument — `Route::BookListen {
/// file_id: None }`, `Route::Settings { section: None }` — renders its href
/// as `/listen/<uuid>?`, which looks broken in a URL a reader copies or
/// bookmarks. Trimming it changes nothing about routing: the router parses a
/// missing query and an empty one identically. Use this anywhere such a
/// route becomes a `Link`'s `to` or a `Navigator::push`.
pub fn link_target(route: Route) -> NavigationTarget {
    let url = route.to_string();
    match url.strip_suffix('?') {
        Some(trimmed) => NavigationTarget::Internal(trimmed.to_string()),
        None => NavigationTarget::Internal(url),
    }
}

/// Where a "Continue" affordance resumes: the player (carrying the point's
/// `book_file_id` as `?file_id=`, since a bare `/listen/:uuid` opens the
/// first audio file) or the reader.
pub fn resume_route(point: &omnibus_shared::ResumePoint) -> Route {
    let uuid = point.record.book_uuid.clone();
    match point.record.format {
        omnibus_shared::ProgressFormat::Audio => Route::BookListen {
            uuid,
            file_id: point.record.book_file_id,
        },
        // Comics reuse the Epub-format progress record (see
        // `omnibus_shared::comic_page_anchor`), so the format alone can't
        // pick the reader — a CBZ-only book resumes into the pager, while
        // anything with a real EPUB keeps the epub.js reader.
        omnibus_shared::ProgressFormat::Epub => {
            let formats = &point.book.formats;
            let has = |ext: &str| formats.iter().any(|f| f.eq_ignore_ascii_case(ext));
            if has("cbz") && !has("epub") {
                Route::ComicRead { uuid }
            } else {
                Route::BookRead { uuid }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::{ProgressFormat, ProgressRecord, ResumePoint};

    use super::*;

    fn point(format: ProgressFormat, book_file_id: Option<i64>) -> ResumePoint {
        ResumePoint {
            record: ProgressRecord {
                book_uuid: "book-a".into(),
                format,
                epub_cfi: None,
                audio_position_seconds: None,
                book_file_id,
                progress_percent: None,
                kobo_location: None,
                updated_at: 0,
                client_updated_at: 0,
            },
            book: Default::default(),
            linked: false,
            cross_format: None,
            total_duration_seconds: None,
            chapter_number: None,
            chapter_count: None,
            playback_rate: None,
        }
    }

    #[test]
    fn link_target_drops_the_separator_when_no_query_argument_is_set() {
        // The reported Files-section Listen href: `/listen/book-a?`.
        assert_eq!(
            link_target(Route::BookListen {
                uuid: "book-a".into(),
                file_id: None,
            }),
            NavigationTarget::Internal("/listen/book-a".into())
        );
        assert_eq!(
            link_target(Route::Settings { section: None }),
            NavigationTarget::Internal("/settings".into())
        );
    }

    #[test]
    fn link_target_keeps_a_query_argument_that_is_set() {
        assert_eq!(
            link_target(Route::BookListen {
                uuid: "book-a".into(),
                file_id: Some(917),
            }),
            NavigationTarget::Internal("/listen/book-a?file_id=917".into())
        );
        assert_eq!(
            link_target(Route::Settings {
                section: Some("library".into()),
            }),
            NavigationTarget::Internal("/settings?section=library".into())
        );
    }

    #[test]
    fn link_target_leaves_a_route_with_no_query_segment_alone() {
        assert_eq!(
            link_target(Route::BookRead {
                uuid: "book-a".into(),
            }),
            NavigationTarget::Internal("/read/book-a".into())
        );
    }

    #[test]
    fn resume_route_sends_a_cbz_only_book_to_the_comic_pager() {
        let mut p = point(ProgressFormat::Epub, None);
        p.book.formats = vec!["CBZ".into()];
        assert_eq!(
            resume_route(&p),
            Route::ComicRead {
                uuid: "book-a".into()
            }
        );
    }

    #[test]
    fn resume_route_keeps_the_epub_reader_when_a_book_has_both_formats() {
        let mut p = point(ProgressFormat::Epub, None);
        p.book.formats = vec!["EPUB".into(), "CBZ".into()];
        assert_eq!(
            resume_route(&p),
            Route::BookRead {
                uuid: "book-a".into()
            }
        );
    }

    #[test]
    fn resume_route_carries_the_audio_file_the_position_was_taken_in() {
        assert_eq!(
            resume_route(&point(ProgressFormat::Audio, Some(917))),
            Route::BookListen {
                uuid: "book-a".into(),
                file_id: Some(917),
            }
        );
    }

    #[test]
    fn resume_route_leaves_the_file_open_when_the_point_names_none() {
        assert_eq!(
            resume_route(&point(ProgressFormat::Audio, None)),
            Route::BookListen {
                uuid: "book-a".into(),
                file_id: None,
            }
        );
    }

    #[test]
    fn resume_route_sends_an_epub_position_to_the_reader() {
        assert_eq!(
            resume_route(&point(ProgressFormat::Epub, None)),
            Route::BookRead {
                uuid: "book-a".into()
            }
        );
    }
}
