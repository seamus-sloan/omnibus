//! Read-only half-star rating display. One renderer for every surface that
//! *shows* a rating without editing it, so a 4.5 draws as four and a half
//! stars wherever it appears rather than rounding to five on one page and not
//! another (#2467). The interactive widget in `pages::book_detail::rating`
//! keeps its own markup — it carries per-half click targets this has no use
//! for — but fills its stars by the same 0/50/100 rule.

use dioxus::prelude::*;

/// How much of the star in `slot` (1..=5) the value `stars` fills, as the
/// percentage width the foreground glyph is clipped to. Half stars land on
/// 50; anything between rounds down to the half below, because a bar drawn
/// wider than the rating overstates it.
fn fill_percent(stars: f32, slot: f32) -> u32 {
    if stars >= slot {
        100
    } else if stars >= slot - 0.5 {
        50
    } else {
        0
    }
}

/// Render a star value without a trailing `.0` (`4.5` stays, `4.0` -> `4`).
pub fn fmt_stars(v: f32) -> String {
    if v.fract().abs() < f32::EPSILON {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// Five stars filled to `stars`, labelled for assistive tech and hidden from
/// it glyph by glyph. `extra_class` is appended to the wrapper so a caller
/// keeps its own sizing and colour hooks.
#[component]
pub fn StarRating(stars: f32, #[props(default)] extra_class: Option<String>) -> Element {
    let extra = extra_class.unwrap_or_default();
    rsx! {
        span {
            class: "star-rating {extra}",
            aria_label: "{fmt_stars(stars)} out of 5 stars",
            for i in 1..=5u8 {
                span {
                    key: "{i}",
                    class: "star-slot",
                    aria_hidden: "true",
                    span { class: "star-bg", "\u{2605}" }
                    span {
                        class: "star-fg",
                        style: "width: {fill_percent(stars, f32::from(i))}%",
                        "\u{2605}"
                    }
                }
            }
        }
    }
}

// SSR markup assertions, so the module is `server`-gated — under `web` its
// contents would be dead code and CI lints with `-D warnings`.
#[cfg(all(test, feature = "server"))]
mod tests;
