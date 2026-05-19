use dioxus::prelude::*;

/// One decorative book spine rendered behind the tagline. Pre-login pages
/// can't query book metadata (the user isn't authenticated), so the spine
/// row uses a static curated palette.
struct Spine {
    title: &'static str,
    accent: &'static str,
    ink: &'static str,
}

const SPINES: &[Spine] = &[
    Spine {
        title: "Piranesi",
        accent: "oklch(0.66 0.13 245)",
        ink: "#f6f7fb",
    },
    Spine {
        title: "Pachinko",
        accent: "oklch(0.70 0.13 95)",
        ink: "#221d10",
    },
    Spine {
        title: "The Overstory",
        accent: "oklch(0.55 0.11 145)",
        ink: "#f3f6ee",
    },
    Spine {
        title: "Sea of Tranquility",
        accent: "oklch(0.74 0.08 220)",
        ink: "#0e1a22",
    },
    Spine {
        title: "Babel",
        accent: "oklch(0.55 0.14 35)",
        ink: "#fbf2eb",
    },
    Spine {
        title: "Hyperion",
        accent: "oklch(0.45 0.15 295)",
        ink: "#f0eaf6",
    },
    Spine {
        title: "Klara and the Sun",
        accent: "oklch(0.78 0.09 75)",
        ink: "#2a1e0e",
    },
    Spine {
        title: "Tomorrow, and Tomorrow",
        accent: "oklch(0.55 0.12 165)",
        ink: "#eaf6f1",
    },
    Spine {
        title: "Demon Copperhead",
        accent: "oklch(0.60 0.14 30)",
        ink: "#fbece2",
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
                div { class: "auth-shell-spines", aria_hidden: "true",
                    for (i, spine) in SPINES.iter().enumerate() {
                        div {
                            class: "auth-shell-spine auth-shell-spine-{i}",
                            style: "background: {spine.accent}; color: {spine.ink};",
                            div { class: "auth-shell-spine-title", "{spine.title}" }
                        }
                    }
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
