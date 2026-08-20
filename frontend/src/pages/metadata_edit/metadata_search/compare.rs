//! The compare view a selected candidate opens into: the source's record for
//! one edition, next to a way back to the candidate list.
//!
//! Read-only at this stage — moving a value into the form is the
//! per-field-apply ticket's job, and it lands in the rows region below the
//! header without disturbing the hand-off this module owns.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::sources::provider_slug;

/// Rendered for a field the source has no value for.
const EMPTY: &str = "\u{2014}";

/// The selected edition, with the header identifying which source it came
/// from and the control that returns to the results.
#[component]
pub(super) fn ComparePanel(
    edition: ProviderEdition,
    hydrating: bool,
    on_back: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "mes-compare", "data-testid": "mes-compare",
            div { class: "mes-compare-head",
                button {
                    r#type: "button",
                    class: "btn ghost sm",
                    "data-testid": "mes-compare-back",
                    onclick: move |_| on_back.call(()),
                    "\u{2190} Back to results"
                }
                span {
                    class: "mes-badge",
                    "data-testid": "mes-compare-source",
                    "data-source": "{provider_slug(edition.source)}",
                    "{edition.source.display_name()}"
                }
                if hydrating {
                    span { class: "mono mes-compare-busy", role: "status", "data-testid": "mes-compare-busy",
                        "Fetching the full record\u{2026}"
                    }
                }
            }
            SourceRecord { edition: edition.clone() }
        }
    }
}

/// Every field this source carries for the selected edition. A field the
/// source knows nothing about renders as an em dash rather than being left
/// out, so the reader can tell "this source has no publisher" from "this
/// screen forgot to show one".
#[component]
fn SourceRecord(edition: ProviderEdition) -> Element {
    let rows: Vec<(&'static str, String)> = vec![
        ("Title", edition.title.clone()),
        ("Author(s)", edition.authors.join(", ")),
        ("Publisher", opt(edition.publisher.as_deref())),
        ("Published", opt(edition.year.as_deref())),
        ("Series", opt(edition.series.as_deref())),
        ("ISBN-13", edition.isbn13.clone()),
        ("ISBN-10", opt(edition.isbn10.as_deref())),
        (
            "Print Pages",
            edition.pages.map(|p| p.to_string()).unwrap_or_default(),
        ),
        ("Genres", edition.genres.join(", ")),
        ("Description", opt(edition.description.as_deref())),
    ];
    rsx! {
        dl { class: "mes-record", "data-testid": "mes-record",
            for (label , value) in rows {
                SourceRecordRow { label, value }
            }
        }
    }
}

/// One label/value pair. `data-testid` is derived from the label so a spec
/// addresses the row by the field it shows.
#[component]
fn SourceRecordRow(label: &'static str, value: String) -> Element {
    let testid = format!("mes-record-{}", super::field_slug(label));
    let filled = !value.trim().is_empty();
    let shown = if filled { value } else { EMPTY.to_string() };
    let class = if filled {
        "mes-record-value"
    } else {
        "mes-record-value mes-record-empty"
    };
    rsx! {
        div { class: "mes-record-row",
            dt { class: "mes-record-label", "{label}" }
            dd { class: "{class}", "data-testid": "{testid}", "{shown}" }
        }
    }
}

/// Trim an optional provider string down to "has visible content", so a
/// whitespace-only value reads as absent rather than as a blank row.
fn opt(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_treats_a_whitespace_only_provider_value_as_absent() {
        assert_eq!(opt(Some("  ")), "");
        assert_eq!(opt(None), "");
        assert_eq!(opt(Some("  Penguin ")), "Penguin");
    }
}
