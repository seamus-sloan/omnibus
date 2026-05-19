use dioxus::prelude::*;

/// One decorative book rendered on the shelf above the tagline. Pre-login
/// pages can't query book metadata (the user isn't authenticated), so the
/// shelf uses a static curated palette of accent colors. Heights / widths
/// vary per slot in CSS for the staggered shelf look.
struct Spine {
    accent: &'static str,
}

const SPINES: &[Spine] = &[
    Spine {
        accent: "oklch(0.66 0.13 245)",
    },
    Spine {
        accent: "oklch(0.70 0.13 95)",
    },
    Spine {
        accent: "oklch(0.55 0.11 145)",
    },
    Spine {
        accent: "oklch(0.74 0.08 220)",
    },
    Spine {
        accent: "oklch(0.55 0.14 35)",
    },
    Spine {
        accent: "oklch(0.45 0.15 295)",
    },
    Spine {
        accent: "oklch(0.78 0.09 75)",
    },
    Spine {
        accent: "oklch(0.55 0.12 165)",
    },
    Spine {
        accent: "oklch(0.60 0.14 30)",
    },
];

/// Split-pane wrapper used by every auth screen. The left pane is the
/// branded art panel; the right pane hosts the form column.
///
/// Props:
/// - `kicker` — small uppercase label above the title (e.g. `"Sign in"`).
/// - `title` — heading rsx for the right pane. Pass plain text or rich
///   markup (e.g. an italic span).
/// - `lede` — optional supporting copy under the title.
/// - `children` — the form content.
#[component]
pub fn AuthShell(
    kicker: String,
    title: Element,
    #[props(default)] lede: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "auth-shell-grid",
            aside { class: "auth-shell-art",
                div { class: "auth-shell-brand",
                    div { class: "auth-shell-brand-mark" }
                    div { class: "auth-shell-brand-word", "Omnibus" }
                }
                div { class: "auth-shell-shelf", aria_hidden: "true",
                    div { class: "auth-shell-spines",
                        for (i, spine) in SPINES.iter().enumerate() {
                            div {
                                class: "auth-shell-spine auth-shell-spine-{i}",
                                style: "background: {spine.accent};",
                                span { class: "auth-shell-spine-dot" }
                            }
                        }
                    }
                    div { class: "auth-shell-shelf-plank" }
                }
                div { class: "auth-shell-tagline",
                    h1 { class: "auth-shell-headline",
                        "Your "
                        span { class: "auth-shell-headline-em", "shelf" }
                        ", anywhere."
                    }
                    p { class: "auth-shell-blurb",
                        "One library for ebooks, audiobooks, journals and quotes — synced across the devices you already own."
                    }
                    div { class: "auth-shell-meta",
                        span { "self-hosted" }
                        span { "·" }
                        span { "open source" }
                        span { "·" }
                        span { "made with dioxus" }
                    }
                }
            }
            section { class: "auth-shell-form",
                div { class: "auth-shell-form-inner",
                    div { class: "auth-shell-kicker", "{kicker}" }
                    h2 { class: "auth-shell-title", {title} }
                    if let Some(text) = lede {
                        p { class: "auth-shell-lede", "{text}" }
                    }
                    div { class: "auth-shell-body", {children} }
                }
            }
        }
    }
}
