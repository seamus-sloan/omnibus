use dioxus::prelude::*;

/// Severity tier for a [`Banner`]. Drives the color, icon, and ARIA role
/// the banner renders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BannerKind {
    Err,
    Warn,
    Info,
    Ok,
}

impl BannerKind {
    /// Suffix for the `auth-banner-<kind>` class.
    pub fn class_suffix(self) -> &'static str {
        match self {
            BannerKind::Err => "err",
            BannerKind::Warn => "warn",
            BannerKind::Info => "info",
            BannerKind::Ok => "ok",
        }
    }

    /// One-character glyph rendered in the banner's icon slot.
    pub fn icon(self) -> &'static str {
        match self {
            BannerKind::Err | BannerKind::Warn => "!",
            BannerKind::Info => "i",
            BannerKind::Ok => "✓",
        }
    }

    /// ARIA role for assistive tech. Errors are `alert` so they
    /// interrupt; warn/info/ok are `status` (polite live region).
    pub fn aria_role(self) -> &'static str {
        match self {
            BannerKind::Err => "alert",
            BannerKind::Warn | BannerKind::Info | BannerKind::Ok => "status",
        }
    }
}

/// Top-of-form callout. Use [`BannerKind`] to pick severity; `title` is
/// required, `message` and `action` are optional.
///
/// `dismissible` renders an inline dismiss button — purely visual here;
/// callers are responsible for hiding the banner in response to the
/// click. (The primitive stays stateless so the same component works
/// under SSR.)
#[component]
pub fn Banner(
    kind: BannerKind,
    title: String,
    #[props(default)] message: Option<String>,
    #[props(default)] action: Option<Element>,
    #[props(default = false)] dismissible: bool,
    #[props(default)] on_dismiss: Option<EventHandler<MouseEvent>>,
) -> Element {
    let kind_class = kind.class_suffix();
    let wrapper_class = format!("auth-banner auth-banner-{kind_class}");
    let icon = kind.icon();
    let role = kind.aria_role();

    rsx! {
        div { class: "{wrapper_class}", role: "{role}",
            div { class: "auth-banner-icon", "{icon}" }
            div { class: "auth-banner-body",
                div { class: "auth-banner-title", "{title}" }
                if let Some(text) = message {
                    div { class: "auth-banner-message", "{text}" }
                }
                if let Some(slot) = action {
                    div { class: "auth-banner-action", {slot} }
                }
            }
            if dismissible {
                button {
                    class: "auth-banner-dismiss",
                    r#type: "button",
                    aria_label: "Dismiss",
                    onclick: move |evt| {
                        if let Some(handler) = on_dismiss.as_ref() {
                            handler.call(evt);
                        }
                    },
                    "✕"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_suffix_covers_every_kind() {
        assert_eq!(BannerKind::Err.class_suffix(), "err");
        assert_eq!(BannerKind::Warn.class_suffix(), "warn");
        assert_eq!(BannerKind::Info.class_suffix(), "info");
        assert_eq!(BannerKind::Ok.class_suffix(), "ok");
    }

    #[test]
    fn error_uses_alert_role_others_use_status() {
        assert_eq!(BannerKind::Err.aria_role(), "alert");
        assert_eq!(BannerKind::Warn.aria_role(), "status");
        assert_eq!(BannerKind::Info.aria_role(), "status");
        assert_eq!(BannerKind::Ok.aria_role(), "status");
    }

    #[test]
    fn icon_glyphs_match_kind() {
        assert_eq!(BannerKind::Err.icon(), "!");
        assert_eq!(BannerKind::Warn.icon(), "!");
        assert_eq!(BannerKind::Info.icon(), "i");
        assert_eq!(BannerKind::Ok.icon(), "✓");
    }
}
