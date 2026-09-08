use super::*;
use crate::test_support::render;

#[test]
fn fill_percent_half_fills_the_star_a_half_rating_lands_on() {
    assert_eq!(fill_percent(4.5, 4.0), 100);
    assert_eq!(fill_percent(4.5, 5.0), 50);
    assert_eq!(fill_percent(4.0, 5.0), 0);
}

#[test]
fn fill_percent_rounds_a_partial_star_down_to_the_half_below() {
    assert_eq!(fill_percent(4.9, 5.0), 50);
    assert_eq!(fill_percent(4.4, 5.0), 0);
}

#[test]
fn fmt_stars_drops_a_trailing_zero_and_keeps_a_half() {
    assert_eq!(fmt_stars(4.0), "4");
    assert_eq!(fmt_stars(4.5), "4.5");
}

// Regression for #2467: the finished list drew a 4.5 as five whole stars.
#[test]
fn star_rating_renders_a_half_filled_fifth_star_for_four_and_a_half() {
    let html = render(rsx! {
        StarRating { stars: 4.5, extra_class: "st-finished-stars".to_string() }
    });
    assert!(
        html.contains(r#"aria-label="4.5 out of 5 stars""#),
        "{html}"
    );
    assert!(html.contains("width: 50%"), "{html}");
    assert!(html.contains("st-finished-stars"), "{html}");
}
