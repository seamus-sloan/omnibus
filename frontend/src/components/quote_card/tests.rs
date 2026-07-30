//! Tests for the shared quote-card editor: SSR render of the panel and the
//! luminance-based ink picker behind the custom background swatch.

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
fn ink_for_picks_dark_ink_on_light_backgrounds_and_light_on_dark() {
    assert_eq!(ink_for("#ffffff"), "#221d18");
    assert_eq!(ink_for("#000000"), "#f5f0e8");
}

#[test]
fn ink_for_falls_back_to_light_ink_on_malformed_hex() {
    assert_eq!(ink_for("nope"), "#f5f0e8");
}
