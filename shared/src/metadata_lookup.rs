//! Wire types for book-metadata resolution against external providers. The
//! check-in ladder answers with one normalized [`ExternalBookMeta`]; the
//! metadata editor's fan-out search answers with an attributed, un-collapsed
//! list of [`ProviderEdition`] plus a per-source [`ProviderSearchStatus`].

use serde::{Deserialize, Serialize};

use crate::scan::SEARCH_QUERY_MAX_LEN;

/// Which external provider a lookup resolved against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataProvider {
    OpenLibrary,
    GoogleBooks,
    /// Only reachable on an instance with a Hardcover API key configured.
    Hardcover,
}

impl MetadataProvider {
    /// Human-readable provider name for display (check-in note, settings panel, provider catalog).
    pub fn display_name(self) -> &'static str {
        match self {
            MetadataProvider::OpenLibrary => "Open Library",
            MetadataProvider::GoogleBooks => "Google Books",
            MetadataProvider::Hardcover => "Hardcover",
        }
    }
}

/// What one provider can be asked and what it can return, independent of
/// whether it is currently configured.
///
/// A flag is a claim about the provider's API, not about one book: a source
/// that can answer for genres advertises `carries_genres` even when a given
/// edition has none. Flip one only in the same change that teaches the
/// provider module to parse the field onto [`ProviderEdition`] — the two are
/// cross-checked against a fixture, so a flag that outruns its parser fails
/// the suite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub search_by_title: bool,
    pub search_by_isbn: bool,
    pub carries_cover: bool,
    pub carries_ratings: bool,
    pub carries_genres: bool,
}

/// One entry in the provider catalog, built by `providers::catalog` in
/// `omnibus-db` and served by `GET /api/metadata/providers`.
///
/// **Never carries key material** — `configured` is a bool, not a masked
/// preview, because this endpoint is reachable by any authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: MetadataProvider,
    pub display_name: String,
    /// Whether the ladder would actually invoke this provider right now —
    /// always `true` for a keyless-capable provider, key-gated otherwise.
    /// Mirrors the same check `providers::ladder` uses, so the two can never
    /// disagree about what "configured" means.
    pub configured: bool,
    /// Whether an API key is required to reach this provider at all (as
    /// opposed to optional — present or not, the provider still answers).
    pub requires_key: bool,
    pub capabilities: ProviderCapabilities,
}

/// Maximum byte length of a stored Google Books API key. Google keys are short
/// (~39 chars), so this is a generous guard against an unbounded blob.
pub const GOOGLE_BOOKS_API_KEY_MAX_LEN: usize = 512;

/// Masked status of the server-wide Google Books key for the Settings UI.
/// **Never carries the raw key** — only a short masked preview. Mirrors
/// [`crate::suggestion::HardcoverKeyStatus`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleBooksKeyStatus {
    pub configured: bool,
    /// Short masked preview (e.g. `AIza…9f3a`), or `None` when unset.
    pub masked: Option<String>,
    /// Where the effective key comes from: `"settings"`, `"env"`, or `"none"`.
    pub source: String,
}

/// Normalized book metadata resolved from an external provider. Every field but
/// `isbn13`, `title`, and `source` is best-effort — providers disagree on which
/// they carry. `cover_url` is a provider-hosted image the caller fetches only
/// when a book/wishlist entry is actually created, not at lookup time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalBookMeta {
    /// The normalized ISBN-13 that resolved (always 13 digits, no hyphens).
    pub isbn13: String,
    pub title: String,
    pub authors: Vec<String>,
    /// Publication year or date as the provider reports it (free text).
    pub year: Option<String>,
    pub pages: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Series statement as the provider reports it (free text, e.g.
    /// "The Kingkiller Chronicle (1)"). Best-effort Open Library enrichment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    /// Year the *work* was first published, across all editions — distinct
    /// from `year`, which is this edition's own date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_publish_year: Option<i64>,
    pub source: MetadataProvider,
}

impl ExternalBookMeta {
    /// Maximum title length in Unicode scalar values (chars). Mirrors
    /// `MetadataOverrides::TITLE_MAX_LEN`.
    pub const TITLE_MAX_LEN: usize = 500;
    /// Maximum length in chars for single-line name fields: publisher, year,
    /// and each author name. Mirrors `MetadataOverrides::NAME_MAX_LEN`.
    pub const NAME_MAX_LEN: usize = 250;
    /// Maximum length (in chars) for the `description` field. Mirrors
    /// `MetadataOverrides::DESCRIPTION_MAX_LEN`.
    pub const DESCRIPTION_MAX_LEN: usize = 50_000;
    /// Maximum **byte** length for `cover_url`. Mirrors
    /// `crate::AUTHOR_PHOTO_URL_MAX_LEN`, which is itself a byte cap — unlike
    /// every other field cap on this type, `cover_url` is validated with
    /// `.len()` (bytes), not `.chars().count()`.
    pub const COVER_URL_MAX_LEN: usize = crate::AUTHOR_PHOTO_URL_MAX_LEN;
    /// Maximum number of authors.
    pub(crate) const MAX_AUTHORS: usize = 32;

    /// Validate field lengths. This is untrusted wire input — either resolved
    /// from an external provider or hand-edited by the client before being
    /// submitted back (add-physical-only / wishlist-by-meta) — so every
    /// free-text field gets a cap before it reaches a DB write. Returns `Err`
    /// with a human-readable message on the first field that exceeds its cap.
    ///
    /// Length caps are measured in Unicode scalar values (`chars`), not UTF-8
    /// bytes, matching `MetadataOverrides::validate` — **except** `cover_url`,
    /// whose cap is byte-based (see [`Self::COVER_URL_MAX_LEN`]).
    pub fn validate(&self) -> Result<(), String> {
        if self.title.chars().count() > Self::TITLE_MAX_LEN {
            return Err(format!("title exceeds {} characters", Self::TITLE_MAX_LEN));
        }
        if self.authors.len() > Self::MAX_AUTHORS {
            return Err(format!("too many authors (max {})", Self::MAX_AUTHORS));
        }
        for author in &self.authors {
            if author.chars().count() > Self::NAME_MAX_LEN {
                return Err(format!(
                    "author name exceeds {} characters",
                    Self::NAME_MAX_LEN
                ));
            }
        }
        let check = |name: &str, val: &Option<String>, max: usize| -> Result<(), String> {
            if let Some(v) = val {
                if v.chars().count() > max {
                    return Err(format!("{name} exceeds {max} characters"));
                }
            }
            Ok(())
        };
        check("year", &self.year, Self::NAME_MAX_LEN)?;
        check("publisher", &self.publisher, Self::NAME_MAX_LEN)?;
        check("series", &self.series, Self::NAME_MAX_LEN)?;
        check("description", &self.description, Self::DESCRIPTION_MAX_LEN)?;
        if let Some(ref cover_url) = self.cover_url {
            if cover_url.len() > Self::COVER_URL_MAX_LEN {
                return Err(format!(
                    "cover_url exceeds {} bytes",
                    Self::COVER_URL_MAX_LEN
                ));
            }
        }
        Ok(())
    }
}

/// One candidate from one provider in a fan-out edition search, kept
/// **attributed and un-collapsed**: two editions sharing an ISBN across two
/// sources stay two entries, and so do two printings one source reports
/// separately, because telling editions apart is what the picker is for.
///
/// Deliberately a new type rather than an extension of [`ExternalBookMeta`] —
/// that one is the check-in wire payload, validated and round-tripped back
/// from the client, and keeping its contract frozen is worth more than the
/// reuse. Convert with `From` where a check-in-shaped value is wanted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEdition {
    pub source: MetadataProvider,
    /// Opaque, provider-scoped handle for re-fetching this candidate in full
    /// once it is selected — a thin search hit carries less than the
    /// provider's own record does (Open Library's search docs have no
    /// description, Google Books' do). Omnibus never parses it: it is the
    /// provider's own id where one exists, and `isbn:<isbn13>` on the rare row
    /// that carries none, which every provider can also be re-queried by.
    pub provider_ref: String,
    /// The normalized ISBN-13 that identifies this edition (13 digits, no
    /// hyphens). Required, exactly as on the check-in path: a candidate a
    /// provider can't put an ISBN on isn't an edition anyone can act on.
    pub isbn13: String,
    /// ISBN-10 for the **same** printing as [`Self::isbn13`], when the
    /// provider carries one.
    ///
    /// Only set when the two demonstrably name one edition — providers list
    /// identifiers per *work* as readily as per edition, and a search hit's
    /// ISBN-13 and ISBN-10 can otherwise come from different printings. A
    /// 979-prefixed ISBN-13 has no ISBN-10 at all, so `None` is the normal
    /// answer for a modern edition, not a parse failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn10: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    /// Publication year or date as the provider reports it (free text).
    pub year: Option<String>,
    /// The edition's **printed** length, which is what the `print_pages`
    /// override stores — never `books.page_count`, which is a comic's image
    /// count. `None` when the provider doesn't say; a provider reporting `0`
    /// for "unknown" is normalized to `None` rather than staged over a real
    /// page count.
    pub pages: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub series: Option<String>,
    /// Year the *work* was first published, across all editions — distinct
    /// from `year`, which is this edition's own date.
    pub first_publish_year: Option<i64>,
    /// Genres as this provider labels them, already sanitized and capped at
    /// [`Self::MAX_GENRES`].
    ///
    /// Empty means "this provider has none for this book"; whether a source
    /// can answer for genres *at all* is
    /// [`ProviderCapabilities::carries_genres`], so the picker can leave out
    /// a column no source could ever fill.
    #[serde(default)]
    pub genres: Vec<String>,
}

impl ProviderEdition {
    /// Maximum genres one candidate contributes.
    ///
    /// Far below `MetadataOverrides::MAX_GENRES` (64) on purpose: that bounds
    /// what a book may *store*, this bounds what one source may *propose*.
    /// Open Library's subject lists run to dozens of entries — genre, setting,
    /// and character names mixed together — and a chip editor handed all of
    /// them is unusable.
    pub const MAX_GENRES: usize = 10;

    /// Fill this record's empty fields from `thinner`, keeping every value
    /// this one already carries.
    ///
    /// The merge behind hydrate-on-select: the detail record a provider
    /// serves for one edition is richer in some fields and *poorer* in
    /// others than the search hit that named it — Open Library's edition
    /// record has the publisher and the printing's own page count but no
    /// subjects or first-publish year, and its search doc is the mirror
    /// image. Re-fetching would otherwise blank whatever the list row had
    /// shown, which is the one thing a picker must never do.
    ///
    /// `self` wins every conflict: it is the record fetched *for this
    /// edition*, where the search hit may describe the work.
    pub fn fill_missing_from(&mut self, thinner: &ProviderEdition) {
        // Both sides are checked for content, not just presence. Providers
        // don't consistently trim these, and filling an absent field with a
        // blank one turns "this source didn't say" into "this source said
        // nothing", which renders as a present-but-empty value.
        let fill = |slot: &mut Option<String>, from: &Option<String>| {
            let blank = |v: &Option<String>| v.as_ref().is_none_or(|s| s.trim().is_empty());
            if blank(slot) && !blank(from) {
                slot.clone_from(from);
            }
        };
        fill(&mut self.isbn10, &thinner.isbn10);
        fill(&mut self.year, &thinner.year);
        fill(&mut self.publisher, &thinner.publisher);
        fill(&mut self.description, &thinner.description);
        fill(&mut self.cover_url, &thinner.cover_url);
        fill(&mut self.series, &thinner.series);
        if self.title.trim().is_empty() {
            self.title.clone_from(&thinner.title);
        }
        if self.authors.is_empty() {
            self.authors.clone_from(&thinner.authors);
        }
        if self.pages.is_none() {
            self.pages = thinner.pages;
        }
        if self.first_publish_year.is_none() {
            self.first_publish_year = thinner.first_publish_year;
        }
        if self.genres.is_empty() {
            self.genres.clone_from(&thinner.genres);
        }
    }
}

impl From<ProviderEdition> for ExternalBookMeta {
    /// Narrow a search candidate to the check-in payload, dropping the fields
    /// that type doesn't carry — `provider_ref`, `isbn10`, and `genres`. The
    /// check-in wire contract stays frozen; the picker keeps the rest.
    fn from(e: ProviderEdition) -> Self {
        Self {
            isbn13: e.isbn13,
            title: e.title,
            authors: e.authors,
            year: e.year,
            pages: e.pages,
            publisher: e.publisher,
            description: e.description,
            cover_url: e.cover_url,
            series: e.series,
            first_publish_year: e.first_publish_year,
            source: e.source,
        }
    }
}

/// What one provider contributed to a fan-out search.
///
/// The three cases are the point of the type: "it has nothing", "we never
/// asked it", and "we asked and never got an answer" are different facts, and
/// collapsing them makes an outage read as a miss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderSearchStatus {
    /// The provider answered. `count` is how many of its candidates reached
    /// the returned list — post-cap, so it never overstates what is there.
    Answered { count: usize },
    /// Not usable on this instance (no API key). No request was sent.
    NotConfigured,
    /// Asked, but no answer came back. `message` is the provider call's own
    /// top-level context, never the error chain below it — which is where a
    /// request URL, and so a `?key=`, would render.
    Failed { message: String },
}

/// One row of the per-source status table returned alongside the candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSearchSource {
    pub provider: MetadataProvider,
    pub display_name: String,
    pub status: ProviderSearchStatus,
}

/// Body for `POST /api/metadata/editions/search`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionSearchRequest {
    pub query: String,
    /// Which providers to ask. Absent means every configured one; this is the
    /// hook a provider-filter UI plugs into without a wire change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<MetadataProvider>>,
}

impl EditionSearchRequest {
    /// Reject a blank or oversized `query`, and an explicitly-empty provider
    /// list — which would otherwise search nothing and look like an outage.
    /// Handlers translate `Err(_)` into 400. Mirrors
    /// [`crate::scan::ScanSearchRequest::validate`], including its cap.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("query is required".into());
        }
        if self.query.chars().count() > SEARCH_QUERY_MAX_LEN {
            return Err(format!("query exceeds {SEARCH_QUERY_MAX_LEN} characters"));
        }
        if self.providers.as_ref().is_some_and(|p| p.is_empty()) {
            return Err("providers must name at least one provider when supplied".into());
        }
        Ok(())
    }
}

/// Body for `POST /api/metadata/editions/hydrate` — the second call the
/// picker makes once a candidate is selected, naming one edition rather than
/// a query.
///
/// `provider_ref` is echoed back exactly as the search handed it over;
/// Omnibus never parses it, so it is capped rather than validated for shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionHydrateRequest {
    pub source: MetadataProvider,
    pub provider_ref: String,
    /// The candidate's ISBN-13, which is what every provider can be re-asked
    /// by regardless of what its own handle looks like.
    pub isbn13: String,
}

impl EditionHydrateRequest {
    /// Maximum byte length of an accepted `provider_ref`. Generous against
    /// the longest handle any provider mints (an Open Library work key runs
    /// to a couple of dozen chars) and small enough that an unbounded blob
    /// can't ride in on it.
    pub const PROVIDER_REF_MAX_LEN: usize = 256;

    /// Reject a blank or oversized handle. The ISBN itself is validated by
    /// `normalize_isbn` further down, where a bad check digit reports the
    /// specific failure. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.provider_ref.trim().is_empty() {
            return Err("provider_ref is required".into());
        }
        if self.provider_ref.len() > Self::PROVIDER_REF_MAX_LEN {
            return Err(format!(
                "provider_ref exceeds {} bytes",
                Self::PROVIDER_REF_MAX_LEN
            ));
        }
        if self.isbn13.trim().is_empty() {
            return Err("isbn13 is required".into());
        }
        Ok(())
    }
}

/// Fan-out search results: the candidates every asked provider returned, plus
/// one status row per provider considered — including the ones that were
/// skipped or failed, which is why a caller can render "Hardcover: couldn't
/// reach it" instead of quietly showing a shorter list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionSearchResponse {
    pub editions: Vec<ProviderEdition>,
    pub sources: Vec<ProviderSearchSource>,
}

#[cfg(test)]
mod tests;
