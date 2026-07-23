//! Nav is platform-specific: `TopNav` on web, `BottomNav` on mobile.
//!
//! The `web` and `mobile` features are **mutually exclusive** — enabling
//! both at once (e.g. because a workspace-wide `cargo build` unified them)
//! triggers a `compile_error!`. Build each platform crate with `cargo build
//! -p <crate>` so its exclusive feature set is used.

#[cfg(all(feature = "web", feature = "mobile"))]
compile_error!(
    "omnibus-frontend's `web` and `mobile` features are mutually exclusive. \
     A workspace-wide `cargo build`/`clippy` unifies features and enables both. \
     Build per-crate instead: `cargo build -p omnibus` for the server, \
     `cargo build -p omnibus-mobile` for the mobile app."
);

// The default (web) variant must compile under the `server` feature too so
// SSR markup matches what the WASM client expects to hydrate. Otherwise
// dioxus's hydration walker fails to locate dynamic text/event nodes and
// throws "Cannot set properties of undefined".
#[cfg(not(feature = "mobile"))]
mod top_nav;
#[cfg(not(feature = "mobile"))]
pub use top_nav::TopNav as Nav;

// The bottom tab bar is the mobile-native `Nav`, and also mounts on web
// (hidden by CSS above the phone breakpoint) so phone-width web gets the
// same thumb-reachable section switcher. Compiled on every target; the web
// build renders it via `ScreenLayout` alongside `TopNav`.
mod add_books_sheet;
mod bottom_nav;
#[cfg(feature = "mobile")]
pub use bottom_nav::BottomNav as Nav;
#[cfg(not(feature = "mobile"))]
pub use bottom_nav::BottomNav;

mod format_switcher;
#[cfg(not(feature = "mobile"))]
pub use format_switcher::SendToKindleButton;
#[cfg(not(feature = "mobile"))]
pub use format_switcher::SendToKoboButton;
pub use format_switcher::{BookActionMeta, FormatSwitcher};

// Reusable add/remove chip editor with a substring-match suggestion
// dropdown. See `chip_editor.rs` for the full API.
pub mod chip_editor;
pub use chip_editor::ChipEditor;

// Autocomplete-dropdown machinery (selection state, the dropdown component,
// the substring filter) shared by `chip_editor` and `suggest_field`.
mod suggestion_dropdown;

// Single-value sibling of `chip_editor`'s dropdown, for fields that hold one
// value rather than a list (e.g. the Series field).
pub mod suggest_field;
pub use suggest_field::SuggestField;

pub mod atrium;

// Shared loading/error/not-found states for top-level pages — see
// `page_state.rs` for the role/text contract each one preserves.
pub mod page_state;
pub use page_state::{PageError, PageLoading, PageNotFound};

// Shared cover-tile chrome (`thumb_srcs` + the `CoverTile` wrapper) used by
// the landing grid, shelf detail, and the shelf pickers. Platform-agnostic so
// the same markup ships under web SSR/WASM and mobile native.
pub mod cover_tile;
pub use cover_tile::{CoverTile, CoverTileKind};

// F1.11 follow-up: hover-overlay "edit photo" affordance + modal with
// three actions (paste URL, upload file, scan Open Library). Mounted by
// the author detail hero only — the `/authors` index renders cached
// photos read-only. Lives in components/ rather than pages/ so the
// modal can be lifted to other surfaces (e.g. an admin bulk-edit view)
// without duplicating the form.
pub mod author_photo_edit;

// F5.10 admin "Merge with…" dialog, mounted by the book detail page.
// Web-only: merge is an admin surface and mobile renders no admin
// affordances.
#[cfg(not(feature = "mobile"))]
pub mod merge_dialog;
#[cfg(not(feature = "mobile"))]
pub use merge_dialog::MergeDialog;

// Admin "Delete files…" dialog, mounted by the book detail page. Web-only
// for the same reason as the merge dialog — mobile renders no admin
// affordances.
#[cfg(not(feature = "mobile"))]
pub mod delete_book_dialog;
#[cfg(not(feature = "mobile"))]
pub use delete_book_dialog::DeleteBookDialog;

#[cfg(not(feature = "mobile"))]
pub mod search_palette;

// F0.5 / issue #69 worker-progress indicator. Mounted on the settings
// page today; the data plane is generic so future surfaces (e.g. the
// F5.9 library-cleanup detection trigger) can mount the same primitive
// without changes here.
#[cfg(not(feature = "mobile"))]
pub mod worker_status;

#[cfg(not(feature = "mobile"))]
mod user_menu;

// Auth-page primitives — used by the login / register / first-run /
// recovery screens on every target. Stays platform-agnostic so the same
// markup ships under web SSR, web WASM, and mobile native.
pub mod auth;

// F3.1 shelves: the left-rail shelf list (shared library chrome) and the
// create-shelf modal it mounts. Platform-agnostic so the rail renders the
// same under web SSR/WASM and mobile native.
pub mod create_shelf_modal;
pub mod shelves_rail;
pub use create_shelf_modal::CreateShelfModal;
pub use shelves_rail::{RailActive, ShelvesRail};

// Mobile connectivity pill (offline / syncing status), mounted by the
// mobile `ScreenLayout`. Mobile-only: it reads the offline sync engine.
#[cfg(feature = "mobile")]
pub mod offline_pill;
#[cfg(feature = "mobile")]
pub use offline_pill::OfflinePill;

// Mobile "you're offline" sheet raised when a reader/player route bounces
// because the book has no completed local download. Mobile-only.
#[cfg(feature = "mobile")]
pub mod offline_guard;
#[cfg(feature = "mobile")]
pub use offline_guard::OfflineGuardModal;

// The condition-row editor + live-preview pane shared by shelf creation and
// the "Edit rules" surface.
pub mod shelf_rule_builder;

pub mod edit_shelf_rules_modal;
pub use edit_shelf_rules_modal::EditShelfRulesModal;

// "Fetch Summary" button — pulls a book blurb from Hardcover/OpenLibrary on
// demand. Mounted by the metadata editor (always) and the web book-detail hero
// (when the current summary is sparse). Platform-agnostic so its rsx stays
// hydration-identical; its data calls have mobile stubs.
pub mod fetch_summary;
pub use fetch_summary::FetchSummaryButton;

// Camera barcode scanner (vendored zxing WASM decoder). Mounted by the
// check-in flow; kept here so a future "scan to add" surface can reuse it.
// Platform-agnostic markup — only the camera-opening effect is target-gated.
pub mod barcode_scanner;
pub use barcode_scanner::{BarcodeScanner, ScanStatus};
