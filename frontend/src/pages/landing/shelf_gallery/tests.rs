//! Unit tests for the shelf-gallery pure helpers: mosaic arrangement, the
//! name-hash plate fallback, and the caption meta line.

use omnibus_shared::{ShelfKind, Visibility};

use super::*;

#[test]
fn mosaic_layout_matches_cover_count_and_clamps_above_four() {
    assert_eq!(mosaic_layout(0), MosaicLayout::Empty);
    assert_eq!(mosaic_layout(1), MosaicLayout::One);
    assert_eq!(mosaic_layout(2), MosaicLayout::Two);
    assert_eq!(mosaic_layout(3), MosaicLayout::Three);
    assert_eq!(mosaic_layout(4), MosaicLayout::Four);
    assert_eq!(mosaic_layout(9), MosaicLayout::Four);
}

#[test]
fn fallback_hue_is_stable_and_bounded() {
    assert_eq!(fallback_hue("Space Operas"), fallback_hue("Space Operas"));
    assert_ne!(fallback_hue("Space Operas"), fallback_hue("Cozy Mysteries"));
    for name in ["", "a", "Space Operas", "日本語の棚"] {
        assert!(fallback_hue(name) < 360);
    }
}

#[test]
fn plate_gradient_uses_accent_when_set_and_name_hash_otherwise() {
    let with_accent = plate_gradient(Some("oklch(0.7 0.1 200)"), "Anything");
    assert!(with_accent.contains("oklch(0.7 0.1 200)"));
    assert!(with_accent.contains("color-mix"));

    let hashed = plate_gradient(None, "Space Operas");
    let h = fallback_hue("Space Operas");
    assert!(hashed.contains(&format!(" {h})")));
    // Blank accent falls through to the hash path too.
    assert_eq!(plate_gradient(Some("  "), "Space Operas"), hashed);
}

#[test]
fn shelf_meta_line_composes_count_kind_and_visibility() {
    assert_eq!(
        shelf_meta_line(1, ShelfKind::Manual, Visibility::Private),
        "1 book"
    );
    assert_eq!(
        shelf_meta_line(12, ShelfKind::Smart, Visibility::Public),
        "12 books \u{00b7} Smart \u{00b7} Public"
    );
    assert_eq!(
        shelf_meta_line(0, ShelfKind::Wishlist, Visibility::Public),
        "0 books \u{00b7} Public"
    );
}
