//! One line under the results saying what each source contributed.
//!
//! It exists because a short list has several causes and they are not
//! interchangeable: a provider that answered with nothing, one this instance
//! has no key for, and one that could not be reached all look identical from
//! the list alone. Dropping a failed provider silently is the failure this
//! line prevents — and it is a line rather than a table because that is all
//! the weight the answer deserves once it is not hidden.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::{
    MetadataProvider, ProviderSearchSource, ProviderSearchStatus,
};

/// Stable slug for a provider, used in `data-testid`s and as a sort key, so
/// neither depends on a display name that could be reworded.
pub(super) fn provider_slug(provider: MetadataProvider) -> &'static str {
    match provider {
        MetadataProvider::OpenLibrary => "open-library",
        MetadataProvider::GoogleBooks => "google-books",
        MetadataProvider::Hardcover => "hardcover",
    }
}

/// What one source contributed, in as few words as the distinction allows,
/// and whether it reads as a problem.
fn status_text(status: &ProviderSearchStatus) -> (String, bool) {
    match status {
        ProviderSearchStatus::Answered { count: 0 } => ("nothing".to_string(), false),
        ProviderSearchStatus::Answered { count } => (count.to_string(), false),
        ProviderSearchStatus::NotConfigured => ("not set up".to_string(), false),
        ProviderSearchStatus::Failed { .. } => ("unavailable".to_string(), true),
    }
}

/// The per-source footer. Absent until a search has actually answered, so it
/// never renders as a row of empty labels.
#[component]
pub(super) fn SourceSummary(sources: Vec<ProviderSearchSource>) -> Element {
    if sources.is_empty() {
        return rsx! {};
    }
    rsx! {
        p { class: "mes-sources", "data-testid": "mes-sources", role: "status",
            for source in sources {
                SourceItem { key: "{provider_slug(source.provider)}", source }
            }
        }
    }
}

/// One source's contribution. A provider failure's own message rides along as
/// a `title` so the reader can find out *what* went wrong without the line
/// turning into an error log.
#[component]
fn SourceItem(source: ProviderSearchSource) -> Element {
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
        span {
            class: "{class}",
            "data-testid": "mes-source-{provider_slug(source.provider)}",
            title: "{detail}",
            "{source.display_name} {text}"
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
    fn status_text_is_a_bare_count_when_a_source_answered() {
        assert_eq!(
            status_text(&ProviderSearchStatus::Answered { count: 8 }).0,
            "8"
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
