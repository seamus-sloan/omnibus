//! Typography preference enums, plus their `web_sys`-backed localStorage
//! persistence (web only — see `mobile::prefs_storage` for the WebView
//! counterpart). These are cross-book, per-user reader prefs (typeface,
//! line spacing, margins); the reader page reads them on mount and writes
//! them on every change.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Typeface {
    Editorial,
    Classic,
    Modern,
}

// Unused only in plain SSR builds (no web, no mobile) — both interactive targets convert.
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
impl Typeface {
    pub(crate) fn to_css(self) -> &'static str {
        // Georgia sits ahead of the generic `serif` so a webfont that fails to
        // load in the section iframe degrades to a real book serif, not Times.
        match self {
            Self::Editorial => "'Instrument Serif',Georgia,serif",
            Self::Classic => "'EB Garamond',Georgia,serif",
            Self::Modern => "Georgia,serif",
        }
    }

    pub(crate) fn to_storage(self) -> &'static str {
        match self {
            Self::Editorial => "editorial",
            Self::Classic => "classic",
            Self::Modern => "modern",
        }
    }

    pub(crate) fn from_storage(s: &str) -> Option<Self> {
        match s {
            "editorial" => Some(Self::Editorial),
            "classic" => Some(Self::Classic),
            "modern" => Some(Self::Modern),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LineSpacing {
    Tight,
    Cozy,
    Airy,
}

#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
impl LineSpacing {
    pub(crate) fn to_css(self) -> &'static str {
        match self {
            Self::Tight => "1.4",
            Self::Cozy => "1.7",
            Self::Airy => "2.0",
        }
    }

    pub(crate) fn to_storage(self) -> &'static str {
        match self {
            Self::Tight => "tight",
            Self::Cozy => "cozy",
            Self::Airy => "airy",
        }
    }

    pub(crate) fn from_storage(s: &str) -> Option<Self> {
        match s {
            "tight" => Some(Self::Tight),
            "cozy" => Some(Self::Cozy),
            "airy" => Some(Self::Airy),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Margins {
    Narrow,
    Normal,
    Wide,
}

#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
impl Margins {
    pub(crate) fn to_css(self) -> &'static str {
        match self {
            Self::Narrow => "95%",
            Self::Normal => "80%",
            Self::Wide => "65%",
        }
    }

    pub(crate) fn to_storage(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Normal => "normal",
            Self::Wide => "wide",
        }
    }

    pub(crate) fn from_storage(s: &str) -> Option<Self> {
        match s {
            "narrow" => Some(Self::Narrow),
            "normal" => Some(Self::Normal),
            "wide" => Some(Self::Wide),
            _ => None,
        }
    }
}

/// Single-page vs two-page spread. Maps to epub.js `rendition.spread(...)`:
/// `"none"` forces a single column, `"auto"` lets epub.js pair pages when the
/// viewport is wide enough.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Spread {
    Single,
    Double,
}

#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
impl Spread {
    pub(crate) fn to_css(self) -> &'static str {
        match self {
            Self::Single => "none",
            Self::Double => "auto",
        }
    }

    pub(crate) fn to_storage(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Double => "double",
        }
    }

    pub(crate) fn from_storage(s: &str) -> Option<Self> {
        match s {
            "single" => Some(Self::Single),
            "double" => Some(Self::Double),
            _ => None,
        }
    }
}

#[cfg(feature = "web")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Persist a single reader preference under its `omn.*` key.
#[cfg(feature = "web")]
pub(crate) fn save_reader_pref(key: &str, value: &str) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(key, value);
    }
}

/// Load a single reader preference, returning `None` when unset.
#[cfg(feature = "web")]
pub(crate) fn load_reader_pref(key: &str) -> Option<String> {
    local_storage().and_then(|s| s.get_item(key).ok().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The storage token is the persistence contract: a `to_storage` arm that
    // drifts out of lockstep with its `from_storage` arm silently loses a
    // saved preference on the next reader mount. These round-trips pin every
    // variant of every enum so that drift fails here, not in a browser.

    #[test]
    fn typeface_round_trips_through_storage_for_every_variant() {
        for variant in [Typeface::Editorial, Typeface::Classic, Typeface::Modern] {
            assert_eq!(Typeface::from_storage(variant.to_storage()), Some(variant));
        }
    }

    #[test]
    fn line_spacing_round_trips_through_storage_for_every_variant() {
        for variant in [LineSpacing::Tight, LineSpacing::Cozy, LineSpacing::Airy] {
            assert_eq!(
                LineSpacing::from_storage(variant.to_storage()),
                Some(variant)
            );
        }
    }

    #[test]
    fn margins_round_trips_through_storage_for_every_variant() {
        for variant in [Margins::Narrow, Margins::Normal, Margins::Wide] {
            assert_eq!(Margins::from_storage(variant.to_storage()), Some(variant));
        }
    }

    #[test]
    fn spread_round_trips_through_storage_for_every_variant() {
        for variant in [Spread::Single, Spread::Double] {
            assert_eq!(Spread::from_storage(variant.to_storage()), Some(variant));
        }
    }

    #[test]
    fn from_storage_returns_none_for_unrecognized_token() {
        // One per enum: an unknown stored value must fall through to `None`
        // so the caller can drop to its default rather than mis-restore.
        assert_eq!(Typeface::from_storage("nonsense"), None);
        assert_eq!(LineSpacing::from_storage(""), None);
        assert_eq!(Margins::from_storage("NARROW"), None);
        assert_eq!(Spread::from_storage("triple"), None);
    }
}
