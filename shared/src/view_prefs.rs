//! Library-view preferences (sort/filter/facet state) persisted per library.
//!
//! Lives here — and not in `frontend/` — so a future server-backed per-user
//! prefs endpoint can reuse the shape verbatim. For now persistence is
//! localStorage on web and in-memory on mobile (see `frontend/src/view_prefs.rs`).

use serde::{Deserialize, Serialize};

/// Which list view to render on the library page.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Table,
    Grid,
}

/// Sortable axes exposed in the toolbar / table headers.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    #[default]
    Title,
    Author,
    Series,
    LastUpdated,
    NewestAdded,
}

/// Ascending or descending sort direction for a [`SortKey`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

/// Active filter facets. AND across facet groups; OR within a group.
///
/// Format values are stored lowercase (`"epub"`, `"m4b"`) since the underlying
/// `EbookMetadata.formats` strings vary in case across sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewFilters {
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub series: Vec<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl ViewFilters {
    /// `true` when no facet has any selected value.
    pub fn is_empty(&self) -> bool {
        self.authors.is_empty()
            && self.series.is_empty()
            && self.formats.is_empty()
            && self.tags.is_empty()
    }
}

/// Persisted library-view preference for a single library path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewPrefs {
    pub view_mode: ViewMode,
    pub sort_key: SortKey,
    pub sort_dir: SortDir,
    #[serde(default)]
    pub filters: ViewFilters,
    /// Whether the filter sidebar is open. Defaults to `false`: a brand-new
    /// visitor sees an unobstructed library and opts into filters via the
    /// toolbar's `Filters` toggle. The choice persists per library. At
    /// narrow viewports the sidebar overlays the content as a drawer when
    /// open, so leaving it closed by default avoids popping a panel over
    /// the books on first load.
    #[serde(default)]
    pub filters_open: bool,
}
