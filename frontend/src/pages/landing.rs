use dioxus::prelude::*;
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata, Identifier, RawMeta};

use crate::{data, use_server_url};

/// Landing page — loads the configured ebook library and renders every
/// metadata field the parser was able to pull, plus the raw OPF dump.
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    let mut library = use_signal(EbookLibrary::default);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);

    let url_for_fetch = server_url.clone();
    use_effect(move || {
        let url = url_for_fetch.clone();
        spawn(async move {
            match data::get_ebooks(&url).await {
                Ok(lib) => {
                    library.set(lib);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let lib = library();
    let is_loading = loading();
    let page_error = error();

    rsx! {
        section { class: "card",
            h1 { "Your Library" }
            p { class: "subtitle",
                if let Some(path) = lib.path.as_ref() {
                    "{path}"
                } else {
                    "Configure your ebook library path in Settings."
                }
            }
            if let Some(msg) = page_error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
            if let Some(msg) = lib.error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
        }

        div {
            id: "ebook-grid",
            "data-testid": "ebook-grid",
            class: "ebook-grid",
            if is_loading {
                p { class: "library-empty", "Loading..." }
            } else if lib.books.is_empty() && lib.error.is_none() && page_error.is_none() {
                p { class: "library-empty", "No ebooks found." }
            } else {
                for book in lib.books {
                    EbookCard { key: "{book.filename}", book: book }
                }
            }
        }
    }
}

#[component]
fn EbookCard(book: EbookMetadata) -> Element {
    let display_title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let series_line = match (book.series.as_ref(), book.series_index.as_ref()) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.clone()),
        _ => None,
    };

    rsx! {
        article { class: "ebook-card", "data-testid": "ebook-card",
            div { class: "ebook-cover",
                if let Some(src) = book.cover_image.as_ref() {
                    img { src: "{src}", alt: "Cover of {display_title}" }
                } else {
                    div { class: "ebook-cover-fallback", "No cover" }
                }
            }
            div { class: "ebook-info",
                h2 { class: "ebook-title", "{display_title}" }
                if !book.creators.is_empty() {
                    p { class: "ebook-authors", "by ",
                        {contributor_inline(&book.creators)}
                    }
                }
                if let Some(s) = series_line {
                    p { class: "ebook-series", "Series: {s}" }
                }

                dl { class: "ebook-fields",
                    {field_row("Publisher", book.publisher.as_deref())}
                    {field_row("Published", book.published.as_deref())}
                    {field_row("Modified", book.modified.as_deref())}
                    {field_row("Language", book.language.as_deref())}
                    {field_row("Rights", book.rights.as_deref())}
                    {field_row("Source", book.source.as_deref())}
                    {field_row("Coverage", book.coverage.as_deref())}
                    {field_row("Type", book.dc_type.as_deref())}
                    {field_row("Format", book.dc_format.as_deref())}
                    {field_row("Relation", book.relation.as_deref())}
                    {field_row("EPUB version", book.epub_version.as_deref())}
                    {field_row("Unique ID", book.unique_identifier.as_deref())}
                }

                if !book.identifiers.is_empty() {
                    IdentifierList { items: book.identifiers.clone() }
                }
                if !book.contributors.is_empty() {
                    p { class: "ebook-meta", "Contributors: ",
                        {contributor_inline(&book.contributors)}
                    }
                }
                if !book.subjects.is_empty() {
                    p { class: "ebook-subjects", {book.subjects.join(" · ")} }
                }

                p { class: "ebook-meta ebook-counts",
                    "{book.resource_count} resources · {book.spine_count} spine items · {book.toc_count} toc entries"
                }

                if let Some(desc) = book.description.as_ref() {
                    // EPUB descriptions frequently contain HTML — render as
                    // plain text to avoid injecting arbitrary markup.
                    p { class: "ebook-description", {strip_html(desc)} }
                }

                if let Some(err) = book.error.as_ref() {
                    p { class: "error", "⚠ {err} ({book.filename})" }
                }

                details { class: "ebook-raw",
                    summary { "Raw OPF metadata ({book.raw_metadata.len()})" }
                    RawTable { rows: book.raw_metadata.clone() }
                }
            }
        }
    }
}

#[component]
fn IdentifierList(items: Vec<Identifier>) -> Element {
    rsx! {
        p { class: "ebook-meta", "Identifiers:" }
        ul { class: "ebook-ids",
            for id in items {
                li {
                    if let Some(scheme) = id.scheme.as_ref() {
                        span { class: "ebook-id-scheme", "{scheme}: " }
                    }
                    code { "{id.value}" }
                }
            }
        }
    }
}

#[component]
fn RawTable(rows: Vec<RawMeta>) -> Element {
    rsx! {
        table { class: "raw-meta-table",
            thead {
                tr {
                    th { "Property" }
                    th { "Value" }
                    th { "Refinements" }
                }
            }
            tbody {
                for r in rows {
                    tr {
                        td {
                            code { "{r.property}" }
                            if let Some(lang) = r.lang.as_ref() {
                                span { class: "raw-meta-lang", " @{lang}" }
                            }
                        }
                        td { class: "raw-meta-value", "{r.value}" }
                        td {
                            if r.refinements.is_empty() {
                                span { class: "raw-meta-empty", "—" }
                            } else {
                                ul { class: "raw-meta-refs",
                                    for (k, v) in r.refinements {
                                        li { code { "{k}" } "={v}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn field_row(label: &'static str, value: Option<&str>) -> Element {
    let Some(v) = value.filter(|s| !s.is_empty()) else {
        return rsx! {};
    };
    let v = v.to_string();
    rsx! {
        dt { "{label}" }
        dd { "{v}" }
    }
}

fn contributor_inline(list: &[Contributor]) -> Element {
    let rendered: Vec<String> = list
        .iter()
        .map(|c| match (c.role.as_deref(), c.file_as.as_deref()) {
            (Some(role), _) if !role.is_empty() => format!("{} ({})", c.name, role),
            _ => c.name.clone(),
        })
        .collect();
    rsx! { "{rendered.join(\", \")}" }
}

/// Extremely cheap HTML-tag stripper. EPUB descriptions are frequently raw
/// HTML; we render as text to avoid injecting arbitrary markup into the page.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
