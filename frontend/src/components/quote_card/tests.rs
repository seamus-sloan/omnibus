//! Tests for the shared quote-card editor: SSR render of the panel, the
//! luminance-based ink picker behind the custom background swatch, and the
//! typeface/size token lookups that style the preview.

use super::*;
use crate::test_support::render_in_vdom;

fn panel() -> Element {
    rsx! {
        QuoteCardPanel {
            quote_text: "The Beauty of the House is immeasurable".to_string(),
            author: "Susanna Clarke".to_string(),
            subtitle: "Piranesi".to_string(),
            accent: "#3a3027".to_string(),
            on_close: move |_| {},
        }
    }
}

#[test]
fn quote_card_panel_renders_the_preview_controls_and_export_actions() {
    let html = render_in_vdom(panel);
    assert!(html.contains("Make a quote card"));
    assert!(html.contains("\u{201c}The Beauty of the House is immeasurable\u{201d}"));
    assert!(html.contains("Susanna Clarke"));
    assert!(html.contains("Piranesi"));
    // The accent swatch leads the preset row and seeds the preview background.
    assert!(html.contains("aria-label=\"Book accent\""));
    assert!(html.contains("background:#3a3027"));
    // Ratio chips and the three export actions.
    assert!(html.contains("9:16"));
    assert!(html.contains("data-testid=\"quote-download\""));
    assert!(html.contains("data-testid=\"quote-copy-image\""));
    assert!(html.contains("data-testid=\"quote-share\""));
}

#[test]
fn quote_card_panel_renders_the_typography_controls() {
    let html = render_in_vdom(panel);
    for label in ["Font", "Size", "Style"] {
        assert!(html.contains(label), "missing {label} control");
    }
    for chip in ["Serif", "Sans", "Mono", "Bold", "Italic"] {
        assert!(html.contains(chip), "missing {chip} chip");
    }
    // Defaults: serif face on the card, medium italic body.
    assert!(html.contains("font-family:var(--serif)"));
    assert!(html.contains("font-size:22px; font-weight:400; font-style:italic"));
}

#[test]
fn quote_card_panel_drops_the_composer_action() {
    // The disabled "Open in composer" placeholder was removed — a disabled
    // control that never activates is noise in an otherwise live panel.
    let html = render_in_vdom(panel);
    assert!(!html.contains("composer"));
}

#[test]
fn font_css_maps_every_token_and_falls_back_to_serif() {
    assert_eq!(font_css("serif"), "var(--serif)");
    assert_eq!(font_css("sans"), "var(--sans)");
    assert_eq!(font_css("mono"), "var(--mono)");
    // An unknown token must land on the card's designed default, not panic.
    assert_eq!(font_css("cursive"), "var(--serif)");
}

#[test]
fn size_css_maps_every_token_and_falls_back_to_medium() {
    assert_eq!(size_css("s"), "18px");
    assert_eq!(size_css("m"), "22px");
    assert_eq!(size_css("l"), "27px");
    assert_eq!(size_css("xl"), "22px");
}

#[test]
fn ink_for_picks_dark_ink_on_light_backgrounds_and_light_on_dark() {
    assert_eq!(ink_for("#ffffff"), "#221d18");
    assert_eq!(ink_for("#000000"), "#f5f0e8");
}

#[test]
fn ink_for_falls_back_to_light_ink_on_malformed_hex() {
    assert_eq!(ink_for("nope"), "#f5f0e8");
}
