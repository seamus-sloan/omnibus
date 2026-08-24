//! Unit tests for the physical-collection + wishlist panel's pure helpers,
//! plus SSR render-smoke coverage for its section builders and delete modal.

use super::*;

#[test]
fn time_ago_shows_just_now_for_sub_minute_gap() {
    assert_eq!(time_ago(1_000, 999), "just now");
}

#[test]
fn time_ago_singularizes_one_minute_and_pluralizes_more() {
    assert_eq!(time_ago(1_060, 1_000), "1 minute ago");
    assert_eq!(time_ago(1_000 + 3 * 60, 1_000), "3 minutes ago");
}

#[test]
fn time_ago_pluralizes_hours_and_days() {
    assert_eq!(time_ago(1_000 + 2 * 3600, 1_000), "2 hours ago");
    assert_eq!(time_ago(172_800, 86_400), "1 day ago");
}

#[test]
fn time_ago_clamps_future_timestamps_to_just_now() {
    assert_eq!(time_ago(1_000, 5_000), "just now");
}

#[test]
fn checked_in_label_prefixes_the_relative_phrase() {
    assert_eq!(checked_in_label(1_060, 1_000), "Checked in 1 minute ago");
}

#[test]
fn source_label_covers_every_variant() {
    assert_eq!(source_label(WishlistSource::Scan), "a scan");
    assert_eq!(source_label(WishlistSource::Detail), "this page");
    assert_eq!(source_label(WishlistSource::Manual), "manual entry");
}

#[test]
fn find_a_copy_url_prefers_isbn_when_present() {
    let url = find_a_copy_url(Some("9780451524935"), "1984", "George Orwell");
    assert_eq!(url, "https://www.amazon.com/s?k=9780451524935");
}

#[test]
fn find_a_copy_url_falls_back_to_title_and_author() {
    let url = find_a_copy_url(None, "Brave New World", "Aldous Huxley");
    assert_eq!(
        url,
        "https://www.amazon.com/s?k=Brave%20New%20World%20Aldous%20Huxley"
    );
}

#[test]
fn find_a_copy_url_treats_blank_isbn_as_absent() {
    let url = find_a_copy_url(Some("   "), "Dune", "Frank Herbert");
    assert!(url.ends_with("Dune%20Frank%20Herbert"));
}

#[test]
fn encode_query_keeps_unreserved_and_escapes_the_rest() {
    assert_eq!(encode_query("a-b_c.d~e"), "a-b_c.d~e");
    assert_eq!(encode_query("a b&c"), "a%20b%26c");
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(feature = "server")]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    /// Build the shared wishlist signals a preview needs, seeded to a state.
    fn seeded_phys(entry: Option<WishlistEntry>, loaded: bool) -> PhysSignals {
        PhysSignals {
            wishlist: use_signal(move || entry.clone()),
            loaded: use_signal(move || loaded),
        }
    }

    fn sample_entry() -> WishlistEntry {
        WishlistEntry {
            id: 1,
            user_id: 1,
            book_uuid: "u".to_string(),
            added_at: 0,
            source: WishlistSource::Detail,
        }
    }

    fn slot_identity(has_physical: bool) -> BdBookIdentity {
        BdBookIdentity {
            uuid: "u".to_string(),
            has_physical,
            isbn: None,
            title: "Dune".to_string(),
            author: "Frank Herbert".to_string(),
        }
    }

    /// The panel's first paint, before the post-mount load resolves.
    fn panel_first_paint() -> Element {
        rsx! {
            BdPhysicalPanel {
                uuid: "book-uuid".to_string(),
                is_fileless: false,
                refresh: use_signal(|| 0u32),
                phys: seeded_phys(None, false),
            }
        }
    }

    #[test]
    fn panel_first_paint_is_empty_before_the_post_mount_load() {
        // The load effect hasn't resolved after one rebuild, so `loaded` stays
        // false and the panel emits nothing — exactly what the client hydrates
        // against (rule 07).
        let html = render_in_vdom(panel_first_paint);
        assert!(!html.contains("data-testid=\"bd-physical-panel\""));
    }

    /// A loaded, copy-less book: wishlist state lives in the hero rail, so the
    /// full-width panel renders nothing at all.
    fn panel_loaded_copyless() -> Element {
        rsx! {
            BdPhysicalPanel {
                uuid: "book-uuid".to_string(),
                is_fileless: false,
                refresh: use_signal(|| 0u32),
                phys: seeded_phys(Some(sample_entry()), true),
            }
        }
    }

    #[test]
    fn panel_renders_nothing_for_a_loaded_book_without_copies() {
        let html = render_in_vdom(panel_loaded_copyless);
        assert!(!html.contains("data-testid=\"bd-physical-panel\""));
        assert!(!html.contains("data-testid=\"wishlist-card\""));
    }

    /// Seed a state with one copy and render the physical section directly, so
    /// the loaded markup (pill + copy card) is covered without the async load.
    fn physical_section_preview() -> Element {
        let state = PhysPanelState {
            copies: use_signal(|| {
                vec![PhysicalCopy {
                    id: 1,
                    book_uuid: "u".to_string(),
                    isbn: Some("978".to_string()),
                    added_by_user_id: None,
                    checked_in_at: 0,
                    note: Some("First edition".to_string()),
                }]
            }),
            wishlist: use_signal(|| None),
            busy: use_signal(|| false),
            err: use_signal(|| None),
            editing: use_signal(|| None),
            note_draft: use_signal(String::new),
            delete_target: use_signal(|| None),
            refresh: use_signal(|| 0u32),
        };
        render_physical_section(state, "".to_string(), false, true)
    }

    #[test]
    fn physical_section_renders_pill_copy_card_and_note() {
        let html = render_in_vdom(physical_section_preview);
        assert!(html.contains("data-testid=\"physical-pill\""));
        assert!(html.contains("In your physical collection"));
        assert!(html.contains("data-testid=\"physical-copy-card\""));
        assert!(html.contains("First edition"));
        // Edit-permitted, so both actions render.
        assert!(html.contains("data-testid=\"copy-edit-note\""));
        assert!(html.contains("data-testid=\"copy-delete\""));
    }

    /// The same copies, rendered as W4 rows in the book's copies list.
    fn physical_rows_w4_preview() -> Element {
        let state = PhysPanelState {
            copies: use_signal(|| {
                vec![PhysicalCopy {
                    id: 1,
                    book_uuid: "u".to_string(),
                    isbn: Some("978".to_string()),
                    added_by_user_id: None,
                    checked_in_at: 0,
                    note: Some("First edition".to_string()),
                }]
            }),
            wishlist: use_signal(|| None),
            busy: use_signal(|| false),
            err: use_signal(|| None),
            editing: use_signal(|| None),
            note_draft: use_signal(String::new),
            delete_target: use_signal(|| None),
            refresh: use_signal(|| 0u32),
        };
        render_physical_rows_w4(state, "".to_string(), false, true)
    }

    #[test]
    fn physical_rows_w4_render_a_copy_row_with_its_status_dot_and_actions() {
        let html = render_in_vdom(physical_rows_w4_preview);
        // A copy is a row in the copies list, not a panel of its own.
        assert!(html.contains("class=\"rx-copy\""), "{html}");
        assert!(
            html.contains("data-testid=\"physical-copy-card\""),
            "{html}"
        );
        // The pill's wording moves to the status dot's accessible name.
        assert!(html.contains("data-testid=\"physical-pill\""), "{html}");
        assert!(html.contains("In your physical collection"), "{html}");
        assert!(
            html.contains("data-testid=\"format-badge-physical\""),
            "{html}"
        );
        assert!(html.contains("First edition"), "{html}");
        assert!(html.contains("data-testid=\"copy-edit-note\""), "{html}");
        assert!(html.contains("data-testid=\"copy-delete\""), "{html}");
    }

    /// The rail slot for a wishlisted book (tracking card + actions).
    fn wishlisted_slot_preview() -> Element {
        rsx! {
            BdWishlistRailSlot {
                identity: slot_identity(false),
                phys: seeded_phys(Some(sample_entry()), true),
            }
        }
    }

    #[test]
    fn wishlist_slot_renders_tracking_card_and_find_a_copy_when_wishlisted() {
        let html = render_in_vdom(wishlisted_slot_preview);
        assert!(html.contains("Physical wishlist"));
        assert!(html.contains("data-testid=\"wishlist-card\""));
        assert!(html.contains("Tracking this title"));
        assert!(html.contains("data-testid=\"wishlist-remove\""));
        assert!(html.contains("data-testid=\"find-a-copy\""));
    }

    /// The rail slot for a book that is neither wishlisted nor owned
    /// physically (the add affordance).
    fn add_slot_preview() -> Element {
        rsx! {
            BdWishlistRailSlot {
                identity: slot_identity(false),
                phys: seeded_phys(None, true),
            }
        }
    }

    #[test]
    fn wishlist_slot_offers_add_when_not_wishlisted() {
        let html = render_in_vdom(add_slot_preview);
        assert!(html.contains("data-testid=\"wishlist-add-card\""));
        assert!(html.contains("data-testid=\"add-to-wishlist\""));
    }

    /// The rail slot for a book already in the physical collection.
    fn physical_owned_slot_preview() -> Element {
        rsx! {
            BdWishlistRailSlot {
                identity: slot_identity(true),
                phys: seeded_phys(None, true),
            }
        }
    }

    #[test]
    fn wishlist_slot_renders_nothing_for_a_physically_owned_book() {
        let html = render_in_vdom(physical_owned_slot_preview);
        assert!(!html.contains("Physical wishlist"));
        assert!(!html.contains("data-testid=\"add-to-wishlist\""));
    }

    /// The rail slot before the shared load resolves — must be empty so the
    /// SSR and first-hydration paints match (rule 07).
    fn unloaded_slot_preview() -> Element {
        rsx! {
            BdWishlistRailSlot {
                identity: slot_identity(false),
                phys: seeded_phys(None, false),
            }
        }
    }

    #[test]
    fn wishlist_slot_renders_nothing_before_the_load_resolves() {
        let html = render_in_vdom(unloaded_slot_preview);
        assert!(!html.contains("Physical wishlist"));
        assert!(!html.contains("data-testid=\"wishlist-add-card\""));
    }

    fn state_with_target(last_fileless: bool) -> PhysPanelState {
        let mut delete_target = use_signal(|| None);
        delete_target.set(Some(DeleteTarget {
            copy_id: 7,
            last_fileless,
        }));
        PhysPanelState {
            copies: use_signal(Vec::new),
            wishlist: use_signal(|| None),
            busy: use_signal(|| false),
            err: use_signal(|| None),
            editing: use_signal(|| None),
            note_draft: use_signal(String::new),
            delete_target,
            refresh: use_signal(|| 0u32),
        }
    }

    fn simple_delete_modal_preview() -> Element {
        render_delete_modal(state_with_target(false), "".to_string(), "u".to_string())
    }

    fn last_copy_modal_preview() -> Element {
        render_delete_modal(state_with_target(true), "".to_string(), "u".to_string())
    }

    #[test]
    fn simple_delete_modal_offers_the_i_sold_it_confirm() {
        let html = render_in_vdom(simple_delete_modal_preview);
        assert!(html.contains("data-testid=\"copy-delete-modal\""));
        assert!(html.contains("data-testid=\"copy-delete-confirm\""));
        assert!(html.contains("I sold it"));
    }

    #[test]
    fn last_copy_modal_offers_remove_or_wishlist() {
        let html = render_in_vdom(last_copy_modal_preview);
        assert!(html.contains("data-testid=\"last-copy-modal\""));
        assert!(html.contains("data-testid=\"last-copy-remove\""));
        assert!(html.contains("data-testid=\"last-copy-wishlist\""));
    }
}
