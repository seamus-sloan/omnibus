use dioxus::prelude::*;

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
