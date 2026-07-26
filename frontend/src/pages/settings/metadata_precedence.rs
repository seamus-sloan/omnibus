//! Per-library metadata-source precedence editor. Two independent ordered
//! lists (ebook / audiobook) with up/down reordering — each list's "Save
//! Order" persists to the owning scan root's `metadata_precedence` column
//! via a dedicated RPC pair, kept separate from the path-saving `Settings`
//! form.

use dioxus::prelude::*;
use omnibus_shared::{MetadataSource, DEFAULT_METADATA_PRECEDENCE};

use crate::{data, use_server_url};

/// Human-readable label for a metadata source, in the settings UI order
/// (matches [`DEFAULT_METADATA_PRECEDENCE`], lowest to highest priority).
fn source_label(source: MetadataSource) -> &'static str {
    match source {
        MetadataSource::FolderStructure => "Folder Structure",
        MetadataSource::EmbeddedTags => "Embedded Tags",
        MetadataSource::OpfSidecar => "OPF Sidecar",
        MetadataSource::OmnibusOverrides => "Manual Edits (Overrides)",
        MetadataSource::ProviderMatch => "Provider Match (Hardcover)",
    }
}

/// Card holding both library's precedence lists. Admin-only — gated by the
/// caller, same as [`super::HardcoverKeyField`]/[`super::smtp::SmtpConfigField`].
#[component]
pub fn MetadataPrecedenceField() -> Element {
    rsx! {
        section { class: "card", "data-testid": "metadata-precedence-card",
            h2 { "Metadata Precedence" }
            p { class: "subtitle",
                "Order the sources Omnibus resolves each field from. Lower items win — the last "
                "source in the list that has a value for a field is used."
            }
            PrecedenceList { library: "ebook", label: "Ebook Library" }
            PrecedenceList { library: "audiobook", label: "Audiobook Library" }
        }
    }
}

/// One library's ordered precedence list, loaded on mount and saved
/// independently of the other list and of the main path form.
#[component]
fn PrecedenceList(library: &'static str, label: &'static str) -> Element {
    let server_url = use_server_url();
    let testid = format!("{library}-metadata-precedence");
    let mut order = use_signal(|| DEFAULT_METADATA_PRECEDENCE.to_vec());
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    let load_url = server_url.clone();
    use_effect(move || {
        let url = load_url.clone();
        spawn(async move {
            if let Ok(loaded) = data::get_metadata_precedence(&url, library).await {
                order.set(loaded);
            }
        });
    });

    let save_url = server_url.clone();
    let on_save = move |_| {
        let url = save_url.clone();
        let current = order();
        in_flight.set(true);
        spawn(async move {
            match data::save_metadata_precedence(&url, library, current).await {
                Ok(saved) => {
                    order.set(saved);
                    msg.set(Some("Metadata precedence saved.".to_string()));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(format!("Failed to save metadata precedence: {e}")));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let len = order().len();

    rsx! {
        div { class: "settings-field", "data-testid": "{testid}",
            span { class: "settings-label", "{label}" }
            ol { class: "metadata-precedence-list",
                for (i , source) in order().into_iter().enumerate() {
                    li {
                        key: "{i}",
                        class: "metadata-precedence-item",
                        "data-testid": "{testid}-item-{i}",
                        span { "{source_label(source)}" }
                        button {
                            r#type: "button",
                            class: "btn ghost",
                            disabled: i == 0,
                            "data-testid": "{testid}-up-{i}",
                            "aria-label": "Move {source_label(source)} up",
                            onclick: move |_| {
                                let mut v = order();
                                if i > 0 {
                                    v.swap(i, i - 1);
                                    order.set(v);
                                }
                            },
                            "\u{2191}"
                        }
                        button {
                            r#type: "button",
                            class: "btn ghost",
                            disabled: i + 1 == len,
                            "data-testid": "{testid}-down-{i}",
                            "aria-label": "Move {source_label(source)} down",
                            onclick: move |_| {
                                let mut v = order();
                                if i + 1 < v.len() {
                                    v.swap(i, i + 1);
                                    order.set(v);
                                }
                            },
                            "\u{2193}"
                        }
                    }
                }
            }
            button {
                r#type: "button",
                class: "btn ghost",
                disabled: in_flight(),
                "data-testid": "{testid}-save",
                onclick: on_save,
                "Save Order"
            }
            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "{testid}-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::test_support::render;

    #[test]
    fn precedence_field_renders_both_libraries_seeded_with_the_default_order() {
        let html = render(rsx! {
            MetadataPrecedenceField {}
        });

        assert!(html.contains("data-testid=\"metadata-precedence-card\""));
        assert!(html.contains("Metadata Precedence"));
        // Both per-library lists render, each seeded from
        // `DEFAULT_METADATA_PRECEDENCE` before any load completes.
        assert!(html.contains("data-testid=\"ebook-metadata-precedence\""));
        assert!(html.contains("data-testid=\"audiobook-metadata-precedence\""));
        assert!(html.contains("Ebook Library"));
        assert!(html.contains("Audiobook Library"));
        assert!(html.contains("Folder Structure"));
        assert!(html.contains("Provider Match (Hardcover)"));
        // First row's up control is present and its Save button is rendered.
        assert!(html.contains("aria-label=\"Move Folder Structure up\""));
        assert!(html.contains("Save Order"));
    }

    #[test]
    fn precedence_field_labels_every_metadata_source() {
        let html = render(rsx! {
            MetadataPrecedenceField {}
        });

        for source in DEFAULT_METADATA_PRECEDENCE {
            assert!(
                html.contains(source_label(source)),
                "missing label for {source:?}"
            );
        }
    }
}
