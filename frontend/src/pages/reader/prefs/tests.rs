//! Pins the target-agnostic defaults each `default_*` function feeds into
//! `init_reader_prefs`'s `use_signal` initializers. These run unconditionally
//! on every target (server/web/mobile) — the hydration-safety invariant from
//! `.claude/rules/07-hydration.md` — so a drift here would produce a first-paint
//! mismatch between SSR and the web client.

use super::*;

#[test]
fn default_font_size_matches_the_documented_starting_size() {
    assert_eq!(default_font_size(), 18);
}

#[test]
fn default_typeface_matches_the_documented_starting_typeface() {
    assert_eq!(default_typeface(), Typeface::Modern);
}

#[test]
fn default_line_spacing_matches_the_documented_starting_spacing() {
    assert_eq!(default_line_spacing(), LineSpacing::Cozy);
}

#[test]
fn default_margins_matches_the_documented_starting_margins() {
    assert_eq!(default_margins(), Margins::Normal);
}

#[test]
fn default_justify_is_off_by_default() {
    assert!(!default_justify());
}

#[test]
fn default_spread_matches_the_documented_starting_spread() {
    assert_eq!(default_spread(), Spread::Double);
}

// `Signal::new` needs a Dioxus runtime, so this runs inside a `VirtualDom` (see `contexts::tests`).
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
