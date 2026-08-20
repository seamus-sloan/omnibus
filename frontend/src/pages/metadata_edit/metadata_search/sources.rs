//! The per-source status area under the candidate list.
//!
//! Its whole reason to exist is that a shorter list has several causes and
//! they are not interchangeable: a provider that answered with nothing, one
//! this instance has no key for, and one that could not be reached all
//! produce the same absence of rows. Dropping a failed provider silently is
//! the failure this area prevents.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::{
    MetadataProvider, ProviderSearchSource, ProviderSearchStatus,
};

/// Stable slug for a provider, used in `data-testid`s so a spec can address
/// one source's row without depending on its display name.
pub(super) fn provider_slug(provider: MetadataProvider) -> &'static str {
    match provider {
        MetadataProvider::OpenLibrary => "open-library",
        MetadataProvider::GoogleBooks => "google-books",
        MetadataProvider::Hardcover => "hardcover",
    }
}

/// The status sentence for one source, and whether it reads as a problem.
///
/// Returned as a pair rather than pre-rendered so the caller decides the
/// markup; the wording is what matters here.
fn status_text(status: &ProviderSearchStatus) -> (String, bool) {
    match status {
        ProviderSearchStatus::Answered { count: 0 } => ("no matches".to_string(), false),
        ProviderSearchStatus::Answered { count: 1 } => ("1 edition".to_string(), false),
        ProviderSearchStatus::Answered { count } => (format!("{count} editions"), false),
        ProviderSearchStatus::NotConfigured => ("not configured".to_string(), false),
        ProviderSearchStatus::Failed { .. } => ("unavailable".to_string(), true),
    }
}

/// One line per provider the search considered — including the ones it never
/// asked and the ones that didn't answer.
#[component]
pub(super) fn SourceStatusList(sources: Vec<ProviderSearchSource>) -> Element {
    rsx! {
        ul { class: "mes-sources", "data-testid": "mes-sources", role: "status",
            for source in sources {
                SourceStatusRow { source }
            }
        }
    }
}

/// One provider's status line. The provider failure's own message rides
/// along as a `title` so the reader can find out *what* went wrong without
/// the list turning into an error log.
#[component]
fn SourceStatusRow(source: ProviderSearchSource) -> Element {
    let (text, is_problem) = status_text(&source.status);
    let detail = match &source.status {
        ProviderSearchStatus::Failed { message } => message.clone(),
        _ => String::new(),
    };
    let class = if is_problem {
        "mes-source mes-source-problem"
    } else {
        "mes-source"
    };
    rsx! {
        li {
            class: "{class}",
            "data-testid": "mes-source-{provider_slug(source.provider)}",
            title: "{detail}",
            span { class: "mes-source-name", "{source.display_name}" }
            span { class: "mes-source-sep", aria_hidden: "true", "\u{2014}" }
            span { class: "mes-source-status", "{text}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_reads_differently_for_empty_unconfigured_and_failed() {
        let empty = status_text(&ProviderSearchStatus::Answered { count: 0 }).0;
        let unconfigured = status_text(&ProviderSearchStatus::NotConfigured).0;
        let failed = status_text(&ProviderSearchStatus::Failed {
            message: "timed out".into(),
        })
        .0;
        assert_eq!(empty, "no matches");
        assert_eq!(unconfigured, "not configured");
        assert_eq!(failed, "unavailable");
        // The point of the type: three distinct causes, three distinct reads.
        assert_ne!(empty, unconfigured);
        assert_ne!(unconfigured, failed);
        assert_ne!(empty, failed);
    }

    #[test]
    fn status_text_marks_only_a_failure_as_a_problem() {
        assert!(
            status_text(&ProviderSearchStatus::Failed {
                message: "boom".into()
            })
            .1
        );
        assert!(!status_text(&ProviderSearchStatus::NotConfigured).1);
        assert!(!status_text(&ProviderSearchStatus::Answered { count: 0 }).1);
    }

    #[test]
    fn status_text_singularizes_a_one_edition_answer() {
        assert_eq!(
            status_text(&ProviderSearchStatus::Answered { count: 1 }).0,
            "1 edition"
        );
        assert_eq!(
            status_text(&ProviderSearchStatus::Answered { count: 4 }).0,
            "4 editions"
        );
    }

    #[test]
    fn provider_slug_is_distinct_per_provider() {
        let slugs = [
            provider_slug(MetadataProvider::OpenLibrary),
            provider_slug(MetadataProvider::GoogleBooks),
            provider_slug(MetadataProvider::Hardcover),
        ];
        let mut sorted = slugs.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), slugs.len());
    }
}
