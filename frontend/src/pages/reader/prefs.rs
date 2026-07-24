//! Reader preferences context: a Copy-able bundle of typography signals
//! (theme, typeface, line spacing, margins, justify, font size) plus
//! setters that thread each new value into the epub.js glue and persist
//! it under `omn.*` localStorage keys. `BookReadPage` publishes via
//! `use_context_provider`; the AA panel reads via `use_context`.

use dioxus::prelude::*;

use crate::components::atrium::{persist_theme, Theme};

#[cfg(feature = "mobile")]
use super::mobile::prefs_storage::save_reader_pref_mobile;
#[cfg(any(feature = "web", feature = "mobile"))]
use super::reader_call;
#[cfg(any(feature = "web", feature = "mobile"))]
use super::reader_call_json;
#[cfg(feature = "web")]
use super::typography::save_reader_pref;
use super::typography::{LineSpacing, Margins, Spread, Typeface};

/// Min/max for the AA-panel font-size slider, in CSS px. Kept in sync with
/// the `font_pct` accessor below so the slider thumb tracks the value.
/// `pub(crate)` so `mobile::prefs_storage` can clamp a loaded value to the
/// same bounds without duplicating the constants.
pub(crate) const FONT_SIZE_MIN: i32 = 12;
pub(crate) const FONT_SIZE_MAX: i32 = 32;

/// Copy-able handle to every reader preference plus its setter. Lives in
/// the Dioxus context for the duration of a reader page mount.
#[derive(Copy, Clone)]
pub(crate) struct ReaderPrefs {
    pub theme: Signal<Theme>,
    pub font_size: Signal<i32>,
    pub typeface: Signal<Typeface>,
    pub line_spacing: Signal<LineSpacing>,
    pub margins: Signal<Margins>,
    pub justify: Signal<bool>,
    pub spread: Signal<Spread>,
}

impl ReaderPrefs {
    /// Apply `t` and persist it via the atrium theme writer. Themes are
    /// app-wide (not just reader-scoped) so persistence is shared.
    pub(crate) fn set_theme(self, t: Theme) {
        let mut theme = self.theme;
        theme.set(t);
        persist_theme(t);
    }

    /// Step the font size down one px (clamped to `FONT_SIZE_MIN`), push
    /// to JS, persist to localStorage (web).
    pub(crate) fn decrease_font(self) {
        let mut font_size = self.font_size;
        let next = (*font_size.read() - 1).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        font_size.set(next);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call("setFontSize", &next.to_string());
        #[cfg(feature = "web")]
        save_reader_pref("omn.fontSize", &next.to_string());
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.fontSize", &next.to_string());
    }

    /// Step the font size up one px (clamped to `FONT_SIZE_MAX`), push to
    /// JS, persist to localStorage (web) / WebView localStorage (mobile).
    pub(crate) fn increase_font(self) {
        let mut font_size = self.font_size;
        let next = (*font_size.read() + 1).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        font_size.set(next);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call("setFontSize", &next.to_string());
        #[cfg(feature = "web")]
        save_reader_pref("omn.fontSize", &next.to_string());
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.fontSize", &next.to_string());
    }

    /// Apply a typeface, push to JS, persist to localStorage (web) /
    /// WebView localStorage (mobile).
    pub(crate) fn set_typeface(self, t: Typeface) {
        let mut typeface = self.typeface;
        typeface.set(t);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call_json("setFont", t.to_css());
        #[cfg(feature = "web")]
        save_reader_pref("omn.typeface", t.to_storage());
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.typeface", t.to_storage());
    }

    /// Apply a line-spacing choice, push to JS, persist to localStorage
    /// (web) / WebView localStorage (mobile).
    pub(crate) fn set_line_spacing(self, ls: LineSpacing) {
        let mut line_spacing = self.line_spacing;
        line_spacing.set(ls);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call_json("setLineHeight", ls.to_css());
        #[cfg(feature = "web")]
        save_reader_pref("omn.lineSpacing", ls.to_storage());
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.lineSpacing", ls.to_storage());
    }

    /// Apply a margins choice, push to JS, persist to localStorage (web) /
    /// WebView localStorage (mobile).
    pub(crate) fn set_margins(self, m: Margins) {
        let mut margins = self.margins;
        margins.set(m);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call_json("setMargins", m.to_css());
        #[cfg(feature = "web")]
        save_reader_pref("omn.margins", m.to_storage());
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.margins", m.to_storage());
    }

    /// Apply a page-view (spread) choice, push to JS, persist to
    /// localStorage (web) / WebView localStorage (mobile).
    pub(crate) fn set_spread(self, s: Spread) {
        let mut spread = self.spread;
        spread.set(s);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call_json("setSpread", s.to_css());
        #[cfg(feature = "web")]
        save_reader_pref("omn.spread", s.to_storage());
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.spread", s.to_storage());
    }

    /// Flip the justify toggle and push the new boolean into the JS glue.
    pub(crate) fn toggle_justify(self) {
        let mut justify = self.justify;
        let next = !*justify.read();
        justify.set(next);
        #[cfg(any(feature = "web", feature = "mobile"))]
        reader_call("setJustify", if next { "true" } else { "false" });
        #[cfg(feature = "web")]
        save_reader_pref("omn.justify", if next { "true" } else { "false" });
        #[cfg(feature = "mobile")]
        save_reader_pref_mobile("omn.justify", if next { "true" } else { "false" });
    }

    /// Slider position for the AA-panel size track, expressed as 0–100%.
    pub(crate) fn font_pct(self) -> f32 {
        // Font size is a small i32 slider value in `[FONT_SIZE_MIN,
        // FONT_SIZE_MAX]`; the casts are exact within that range.
        #[allow(clippy::cast_precision_loss)]
        let offset = (*self.font_size.read() - FONT_SIZE_MIN) as f32;
        #[allow(clippy::cast_precision_loss)]
        let span = (FONT_SIZE_MAX - FONT_SIZE_MIN) as f32;
        (offset / span * 100.0).clamp(0.0, 100.0)
    }
}

/// Construct the reader-prefs signals, seeding each from localStorage on
/// web (with a target-agnostic default fallback). Mobile seeds the same
/// defaults here and overwrites them asynchronously once
/// `mobile::prefs_storage::load_and_apply_reader_prefs` resolves the
/// WebView's `localStorage` (see that function's doc for the first-paint
/// timing tradeoff). `theme` is borrowed from the app-wide atrium signal so
/// changes here flip both reader and the surrounding chrome.
pub(crate) fn init_reader_prefs(theme: Signal<Theme>) -> ReaderPrefs {
    let font_size = use_signal(load_font_size_or_default);
    let typeface = use_signal(load_typeface_or_default);
    let line_spacing = use_signal(load_line_spacing_or_default);
    let margins = use_signal(load_margins_or_default);
    let justify = use_signal(load_justify_or_default);
    let spread = use_signal(load_spread_or_default);
    ReaderPrefs {
        theme,
        font_size,
        typeface,
        line_spacing,
        margins,
        justify,
        spread,
    }
}

fn load_font_size_or_default() -> i32 {
    #[cfg(feature = "web")]
    {
        super::typography::load_reader_pref("omn.fontSize")
            .and_then(|s| s.parse::<i32>().ok())
            .map(|n| n.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX))
            .unwrap_or(18)
    }
    #[cfg(not(feature = "web"))]
    18
}

fn load_typeface_or_default() -> Typeface {
    #[cfg(feature = "web")]
    {
        super::typography::load_reader_pref("omn.typeface")
            .and_then(|s| Typeface::from_storage(&s))
            .unwrap_or(Typeface::Editorial)
    }
    #[cfg(not(feature = "web"))]
    Typeface::Editorial
}

fn load_line_spacing_or_default() -> LineSpacing {
    #[cfg(feature = "web")]
    {
        super::typography::load_reader_pref("omn.lineSpacing")
            .and_then(|s| LineSpacing::from_storage(&s))
            .unwrap_or(LineSpacing::Cozy)
    }
    #[cfg(not(feature = "web"))]
    LineSpacing::Cozy
}

fn load_margins_or_default() -> Margins {
    #[cfg(feature = "web")]
    {
        super::typography::load_reader_pref("omn.margins")
            .and_then(|s| Margins::from_storage(&s))
            .unwrap_or(Margins::Normal)
    }
    #[cfg(not(feature = "web"))]
    Margins::Normal
}

fn load_justify_or_default() -> bool {
    #[cfg(feature = "web")]
    {
        super::typography::load_reader_pref("omn.justify").as_deref() == Some("true")
    }
    #[cfg(not(feature = "web"))]
    false
}

fn load_spread_or_default() -> Spread {
    #[cfg(feature = "web")]
    {
        super::typography::load_reader_pref("omn.spread")
            .and_then(|s| Spread::from_storage(&s))
            .unwrap_or(Spread::Double)
    }
    #[cfg(not(feature = "web"))]
    Spread::Double
}

#[cfg(test)]
mod tests {
    use super::*;

    // `Signal::new` requires an active Dioxus runtime, so the assertions run
    // inside a throwaway component driven by a bare `VirtualDom` rebuild —
    // mirrors `contexts::tests::bump_cover_cache_bust_is_observed_by_other_readers_of_the_same_signal`.
    #[test]
    fn font_pct_maps_font_size_bounds_and_midpoint_to_expected_percentage() {
        #[component]
        fn AssertFontPct() -> Element {
            let prefs = ReaderPrefs {
                theme: Signal::new(Theme::Dark),
                font_size: Signal::new(FONT_SIZE_MIN),
                typeface: Signal::new(Typeface::Editorial),
                line_spacing: Signal::new(LineSpacing::Cozy),
                margins: Signal::new(Margins::Normal),
                justify: Signal::new(false),
                spread: Signal::new(Spread::Single),
            };
            assert_eq!(prefs.font_pct(), 0.0, "min font size maps to 0%");

            let mut font_size = prefs.font_size;
            font_size.set(FONT_SIZE_MAX);
            assert_eq!(prefs.font_pct(), 100.0, "max font size maps to 100%");

            font_size.set((FONT_SIZE_MIN + FONT_SIZE_MAX) / 2);
            assert_eq!(prefs.font_pct(), 50.0, "midpoint font size maps to 50%");

            rsx! {}
        }
        VirtualDom::new(AssertFontPct).rebuild_in_place();
    }
}
