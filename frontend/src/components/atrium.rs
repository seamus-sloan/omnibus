//! Atrium design-system primitives: [`AtriumRoot`], [`Cover`], and the
//! [`init_theme`] context provider. The CSS lives in
//! [`frontend/assets/atrium.css`](../../assets/atrium.css); pages compose
//! these markup-only components (no business logic, no data fetching)
//! against those classes.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use crate::contexts::{append_cache_bust, cover_bust_for, use_cover_cache_bust};
use crate::{media_url, use_server_url};

// ── Theme state ────────────────────────────────────────────────────

/// Atrium theme. Persisted under `omn.theme` via [`crate::client_store`]
/// (`localStorage` on web, a JSON file on mobile).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    Sepia,
}

impl Theme {
    /// Map this theme variant to its `data-theme` HTML attribute string
    /// (e.g. `"dark"`, `"light"`, `"sepia"`).
    pub fn as_attr(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Sepia => "sepia",
        }
    }

    /// Parse a `data-theme` attribute string back into a [`Theme`].
    /// Returns `None` for any value outside the known set.
    pub fn from_attr(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            "sepia" => Some(Theme::Sepia),
            _ => None,
        }
    }
}

/// Install the theme signal in the app context. Call once from the root
/// component.
///
/// SSR-safe by construction: the signal always starts as `Theme::Dark` so
/// server-rendered markup is deterministic and matches the first paint on
/// every client target (no hydration mismatch on the `data-theme`
/// attribute). A target-agnostic `use_effect` then reads the persisted value
/// back via [`crate::client_store`] (`localStorage` on web, a JSON file on
/// mobile, always `None` on SSR) after the component mounts and updates the
/// signal if the user has a stored preference, triggering a single
/// re-render to apply it. Mirrors the `view_prefs` hydration pattern in
/// `frontend/src/pages/landing.rs`.
pub fn init_theme() {
    let mut theme = use_context_provider(|| Signal::new(Theme::Dark));
    use_effect(move || {
        if let Some(persisted) = read_persisted_theme() {
            theme.set(persisted);
        }
    });
}

/// Wrap the app body in the Atrium themed container. The `data-theme`
/// attribute is what flips the CSS variable block, so consumers see a
/// re-render rather than a side-effecting DOM mutation.
#[component]
pub fn AtriumRoot(children: Element) -> Element {
    let theme = use_context::<Signal<Theme>>();
    let attr = theme.read().as_attr();
    rsx! {
        div {
            class: "atrium",
            "data-theme": "{attr}",
            {children}
        }
    }
}

// ── Cover ─────────────────────────────────────────────────────────

/// Book cover. Renders the real image when one exists, otherwise a stylized
/// "plate" template using the book's title + author metadata on a neutral
/// grey ground (the per-book accent only tints real-image theming, never
/// the missing-cover plate).
///
/// The `--accent` custom property is set inline from the book's extracted
/// accent color (or left to inherit the page-level default). Consumers who
/// want the accent to bleed past the cover (e.g. detail-page hero backdrop)
/// can read the same value from `book.accent`.
///
/// Props:
/// - `book` — the metadata row driving the cover.
/// - `src_override` — when present, used for the rendered `<img src>`
///   instead of `book.cover_url`. Grid callers point this at the
///   `/api/thumbs/:id/{sm,md,lg}` responsive thumbnail endpoint with the
///   client's `use_server_url()` prefix; the raw `/api/covers/:id` would
///   skip the resized WebP cache and break the mobile origin.
/// - `srcset` / `sizes` — companion responsive-image attributes for the
///   thumbnail variant; ignored when no image is rendered.
#[component]
pub fn Cover(
    book: EbookMetadata,
    #[props(default)] src_override: Option<String>,
    #[props(default)] srcset: Option<String>,
    #[props(default)] sizes: Option<String>,
) -> Element {
    let accent_style = book
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    // Fall back to the filename when the title is absent *or* an empty /
    // whitespace-only string. Without this, books whose `title` is
    // `Some("")` rendered a blank plate (issue #92): `unwrap_or_else` only
    // fires on `None`, so an empty string slipped through as the title.
    let title = fallback_title(book.title.as_deref(), &book.filename);
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let author_label = author
        .split(',')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .next_back()
        .unwrap_or("")
        .to_uppercase();
    // Callers that pass `src_override` (grid/table/rails) already built an
    // authenticated, cache-busted URL themselves (`CoverTile::thumb_srcs`).
    // The `book.cover_url` fallback (detail hero, listen page) is a relative
    // `/api/covers/:uuid` path, so route it through `media_url` to give it an
    // origin + mobile token (no-op on web), then apply this book's cache-bust
    // counter — `/api/covers/*` is cached `private, max-age=86400`, so
    // without it a book detail revisited after a cover edit would keep
    // showing the pre-edit image until the cache expires (issue #1087).
    // `use_cover_cache_bust()` is the context lookup and stays unconditional
    // per the rules of hooks, but the signal *read* (`cover_bust_for`) is
    // gated on `src_override` being absent — reading it unconditionally
    // would subscribe every mounted `Cover` to `CoverCacheBust`, forcing a
    // re-render of the whole grid/table on a single book's cover bump.
    let server_url = use_server_url();
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let cover_cache_bust = use_cover_cache_bust();
    let image_src = match src_override {
        Some(src) => Some(src),
        None => book.cover_url.as_deref().map(|path| {
            let cache_bust = cover_bust_for(cover_cache_bust.0, &uuid);
            append_cache_bust(media_url(&server_url, path), cache_bust)
        }),
    };
    let srcset_attr = srcset.unwrap_or_default();
    let sizes_attr = sizes.unwrap_or_default();
    // Broken/expired cover URLs otherwise render the browser's broken-image
    // icon with no self-heal until a full reload; fall back to the
    // typographic plate on a load error instead. Tracks the *url* that
    // failed (not just a bool) so a later render with a different url —
    // a mobile token rotation, a different `src_override`, or a
    // cache-busted regenerated cover (see `cache_bust` above) — always
    // gets a fresh chance to load, even though this component instance
    // stays mounted across the change.
    let mut broken_cover_src: Signal<Option<String>> = use_signal(|| None);

    rsx! {
        div {
            class: "cover-wrap",
            style: "{accent_style}",
            "data-testid": "cover",
            div {
                class: "cover tpl-plate",
                if let Some(url) =
                    image_src.filter(|u| broken_cover_src.read().as_deref() != Some(u.as_str()))
                {
                    img {
                        src: "{url}",
                        srcset: "{srcset_attr}",
                        sizes: "{sizes_attr}",
                        alt: "Cover of {title}",
                        loading: "lazy",
                        onerror: {
                            let url = url.clone();
                            move |_| broken_cover_src.set(Some(url.clone()))
                        },
                    }
                } else {
                    div { class: "ca", "{author_label}" }
                    div { class: "ct", "{title}" }
                }
            }
        }
    }
}

/// Pick the text for the typographic cover plate: the book's `title` when it
/// carries non-blank content, otherwise the on-disk `filename`. Guards the
/// `Some("")` / whitespace-only case that otherwise renders a blank plate.
pub(crate) fn fallback_title(title: Option<&str>, filename: &str) -> String {
    match title {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => filename.to_string(),
    }
}

// ── Persistence ───────────────────────────────────────────────────

const THEME_STORAGE_KEY: &str = "omn.theme";

/// Persist the chosen theme via [`crate::client_store`]: `localStorage` on
/// web, a JSON file under the app data dir on mobile (survives cold
/// launch), a no-op on SSR.
pub(crate) fn persist_theme(t: Theme) {
    crate::client_store::set(THEME_STORAGE_KEY, t.as_attr());
}

/// Read the persisted theme value back via [`crate::client_store`]. Returns
/// `None` when nothing is stored, storage is unavailable, or the stored
/// value doesn't parse as a known [`Theme`].
fn read_persisted_theme() -> Option<Theme> {
    let value = crate::client_store::get(THEME_STORAGE_KEY)?;
    Theme::from_attr(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_attr_round_trips() {
        assert_eq!(Theme::Dark.as_attr(), "dark");
        assert_eq!(Theme::Light.as_attr(), "light");
        assert_eq!(Theme::Sepia.as_attr(), "sepia");
        assert_eq!(Theme::from_attr("dark"), Some(Theme::Dark));
        assert_eq!(Theme::from_attr("light"), Some(Theme::Light));
        assert_eq!(Theme::from_attr("sepia"), Some(Theme::Sepia));
        assert_eq!(Theme::from_attr("nope"), None);
    }

    #[test]
    fn fallback_title_prefers_a_present_title() {
        assert_eq!(fallback_title(Some("Dune"), "dune.epub"), "Dune");
    }

    #[test]
    fn fallback_title_uses_filename_when_title_is_none() {
        assert_eq!(fallback_title(None, "dune.epub"), "dune.epub");
    }

    #[test]
    fn fallback_title_uses_filename_for_empty_title() {
        // Regression for issue #92: a `Some("")` title rendered a blank plate.
        assert_eq!(fallback_title(Some(""), "dune.epub"), "dune.epub");
    }

    #[test]
    fn fallback_title_uses_filename_for_whitespace_only_title() {
        assert_eq!(fallback_title(Some("   "), "dune.epub"), "dune.epub");
    }
}

// SSR has no persistence: `persist_theme` is inert and `read_persisted_theme`
// always sees nothing, so `init_theme` keeps its deterministic `Theme::Dark`
// default for SSR markup.
#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    #[test]
    fn persist_theme_is_a_noop_and_read_persisted_theme_stays_none() {
        assert_eq!(read_persisted_theme(), None);
        persist_theme(Theme::Sepia);
        assert_eq!(read_persisted_theme(), None);
    }
}
