//! Unit coverage for the picker's pure helpers, plus the hook-order
//! regression the availability gate has to survive (rule 07).

use super::*;

#[test]
fn compose_query_joins_the_title_and_the_primary_author() {
    assert_eq!(
        compose_query("Effective Java", Some("Joshua Bloch")),
        "Effective Java Joshua Bloch"
    );
}

#[test]
fn compose_query_emits_no_stray_space_when_one_half_is_missing() {
    // A book with no author must not seed `"Title "`, which searches as a
    // different string than the title.
    assert_eq!(compose_query("Effective Java", None), "Effective Java");
    assert_eq!(
        compose_query("Effective Java", Some("   ")),
        "Effective Java"
    );
    assert_eq!(compose_query("  ", Some("Joshua Bloch")), "Joshua Bloch");
}

#[test]
fn compose_query_is_empty_when_the_book_has_neither() {
    // Which is what leaves the Search button disabled rather than firing a
    // request the endpoint would 400.
    assert_eq!(compose_query("   ", None), "");
}

fn seed() -> Seed {
    Seed {
        title: Some("Dune".into()),
        author: Some("Frank Herbert".into()),
        isbn13: Some("9780441013593".into()),
    }
}

#[test]
fn request_sends_the_structured_fields_while_the_box_is_untouched() {
    // The overwhelmingly common case, and the one the flattened query got
    // wrong: Open Library must receive `title=Dune&author=Frank+Herbert`, not
    // one phrase searched inside the title field.
    let seed = seed();
    let req = seed.request(&seed.composed());
    assert_eq!(req.title.as_deref(), Some("Dune"));
    assert_eq!(req.author.as_deref(), Some("Frank Herbert"));
    assert_eq!(req.isbn.as_deref(), Some("9780441013593"));
    assert_eq!(req.query, "Dune Frank Herbert");
}

#[test]
fn request_tolerates_surrounding_whitespace_in_an_untouched_box() {
    let seed = seed();
    let req = seed.request(&format!("  {}  ", seed.composed()));
    assert_eq!(req.title.as_deref(), Some("Dune"));
}

#[test]
fn request_falls_back_to_free_text_once_the_reader_edits_the_box() {
    // A reader who retyped it is asking for something specific, and free text
    // cannot honestly be split back into a title and an author — guessing a
    // split would search for something they did not type.
    let req = seed().request("dune messiah");
    assert_eq!(req.query, "dune messiah");
    assert_eq!(req.title, None);
    assert_eq!(req.author, None);
    assert_eq!(req.isbn, None);
}

#[test]
fn request_carries_no_isbn_when_the_form_has_none() {
    let seed = Seed {
        isbn13: None,
        ..seed()
    };
    let req = seed.request(&seed.composed());
    assert_eq!(req.isbn, None);
    assert_eq!(req.title.as_deref(), Some("Dune"));
}

#[test]
fn request_omits_a_title_the_book_does_not_have() {
    // Sent as `Some("")` the server would take the structured branch and then
    // clean it away, searching with nothing.
    let seed = Seed {
        title: None,
        ..seed()
    };
    let req = seed.request(&seed.composed());
    assert_eq!(req.title, None);
    assert_eq!(req.query, "Frank Herbert");
}

#[test]
fn field_slug_matches_the_forms_own_label_slugging_without_its_prefix() {
    assert_eq!(field_slug("Title"), "title");
    assert_eq!(field_slug("ISBN-13"), "isbn-13");
    assert_eq!(field_slug("Author(s)"), "author-s-");
    assert_eq!(field_slug("Print Pages"), "print-pages");
}

// Hook-order regression for the availability gate: a structural stand-in for
// the overlay, which needs a live server context a unit test doesn't have.
#[cfg(all(test, feature = "server"))]
mod gate {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;

    /// Carries the harness's gate signal out to the test so it can be
    /// flipped from outside the component tree, the way the real panel's
    /// post-mount effect flips it after first paint.
    #[derive(Clone)]
    struct Capture(Rc<RefCell<Option<Signal<bool>>>>);

    impl PartialEq for Capture {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    fn gate_root(capture: Capture) -> Element {
        let available = use_signal(|| false);
        use_hook(|| {
            *capture.0.borrow_mut() = Some(available);
        });
        rsx! {
            GateHarness { available }
        }
    }

    /// Mirrors `MetadataSearchPanel`'s fixed shape: the gate signal, then
    /// the overlay's own `open` signal declared unconditionally, then the
    /// early-return gate. Declaring `open` *after* the gate would make an
    /// ungated render call fewer hooks than a revealed one. (Every other
    /// signal moved inside `SearchOverlay`, which is mounted only while open
    /// — a component that never renders ungated can't mismatch.)
    #[component]
    fn GateHarness(available: Signal<bool>) -> Element {
        let open = use_signal(|| false);

        if !available() {
            return rsx! {
                div { "data-testid": "gated" }
            };
        }
        rsx! {
            div { "data-testid": "revealed", "{open()}" }
        }
    }

    #[test]
    fn gate_harness_survives_the_available_transition_without_a_hook_order_panic() {
        let captured = Capture(Rc::new(RefCell::new(None)));
        let mut dom = VirtualDom::new_with_props(gate_root, captured.clone());
        dom.rebuild_in_place();

        let before = dioxus::ssr::render(&dom);
        assert!(before.contains("gated"), "gated before the check resolves");
        assert!(!before.contains("revealed"));

        let mut available = captured
            .0
            .borrow_mut()
            .take()
            .expect("gate_root captures its signal on first mount");
        available.set(true);

        // The operation that would panic on a hook-count mismatch.
        dom.render_immediate_to_vec();

        let after = dioxus::ssr::render(&dom);
        assert!(after.contains("revealed"), "revealed after the transition");
        assert!(!after.contains("gated"));
    }
}
