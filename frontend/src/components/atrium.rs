//! Atrium design-system primitives (F1.7).
//!
//! The CSS lives in [`frontend/assets/atrium.css`](../../assets/atrium.css);
//! this module exposes the Dioxus components that consume those classes.
//! Keep the components markup-only — no business logic, no data fetching.
//! Pages compose them.
//!
//! Components:
//! - [`AtriumRoot`] — wraps the router output in a
//!   `<div class="atrium" data-theme="dark|light|sepia">`. The Atrium token
//!   block in `frontend/assets/atrium.css` keys off the `data-theme`
//!   attribute on this wrapper (not on `<html>`) so the swap is declarative —
//!   no DOM-attribute mutation from Rust.
//! - [`Cover`] — book cover. Uses the real `/api/covers/:id` image when the
//!   book has one and falls back to a stylized typographic template when it
//!   doesn't. The per-book accent (from `EbookMetadata.accent`, populated
//!   by [`omnibus_db::ebook::extract_accent`]) is wired as a `--accent`
//!   custom property so cover-derived theming composes against the page.
//!
//! SSR/hydration: [`init_theme`] always seeds the context with `Theme::Dark`
//! so the SSR-rendered markup is deterministic and matches the WASM
//! client's first paint. A web-only [`use_effect`] reads the persisted value
//! from `localStorage` after hydration and updates the signal, which
//! re-renders [`AtriumRoot`] with the user's stored preference.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

// ── Theme state ────────────────────────────────────────────────────

/// Atrium theme. Persisted under `omn.theme` in localStorage on web.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    Sepia,
}

impl Theme {
    pub fn as_attr(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Sepia => "sepia",
        }
    }

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
/// server-rendered markup is deterministic and matches the WASM client's
/// first paint (no hydration mismatch on the `data-theme` attribute). A
/// web-only `use_effect` then reads `localStorage["omn.theme"]` after the
/// component mounts and updates the signal if the user has a stored
/// preference, triggering a single re-render to apply it.
pub fn init_theme() {
    // Server / mobile builds only need the signal context — `theme` is
    // unused in those targets, hence the `_` prefix to keep clippy quiet
    // under `-D warnings`. The web build shadows it with a mutable binding
    // inside the `#[cfg(feature = "web")]` block below so the
    // post-hydration `use_effect` can write to it.
    let _theme = use_context_provider(|| Signal::new(Theme::Dark));
    #[cfg(feature = "web")]
    {
        let mut theme = _theme;
        use_effect(move || {
            if let Some(persisted) = read_persisted_theme() {
                theme.set(persisted);
            }
        });
    }
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
/// grey ground (the per-book accent only tints real-image theming, never the
/// missing-cover plate — see issue #92).
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
    let image_src = src_override.or_else(|| book.cover_url.clone());
    let srcset_attr = srcset.unwrap_or_default();
    let sizes_attr = sizes.unwrap_or_default();

    rsx! {
        div {
            class: "cover-wrap",
            style: "{accent_style}",
            "data-testid": "cover",
            div {
                class: "cover tpl-plate",
                if let Some(url) = image_src {
                    img {
                        src: "{url}",
                        srcset: "{srcset_attr}",
                        sizes: "{sizes_attr}",
                        alt: "Cover of {title}",
                        loading: "lazy",
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
fn fallback_title(title: Option<&str>, filename: &str) -> String {
    match title {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => filename.to_string(),
    }
}

// ── Persistence ───────────────────────────────────────────────────

#[cfg(feature = "web")]
pub(crate) fn persist_theme(t: Theme) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item("omn.theme", t.as_attr());
    }
}

#[cfg(not(feature = "web"))]
pub(crate) fn persist_theme(_t: Theme) {
    // TODO(F1.7-mobile): persist to $HOME/.omnibus-theme in debug builds,
    // analogous to data::token_store.
}

/// Web-only: read the persisted theme value from `localStorage`. The
/// `init_theme` `use_effect` call site is itself `#[cfg(feature = "web")]`,
/// so this function only exists when the WASM client is being built. No
/// non-web stub — the call is gated, so the symbol is never referenced
/// on server / mobile builds.
#[cfg(feature = "web")]
fn read_persisted_theme() -> Option<Theme> {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten())?;
    let value = storage.get_item("omn.theme").ok().flatten()?;
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
