//! Unit coverage for the picker's pure helpers, plus the hook-order
//! regression the availability gate has to survive (rule 07).

use super::*;

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

// ── the query fields ─────────────────────────────────────────────

#[test]
fn build_request_sends_each_field_as_itself() {
    // The whole point of three fields: Open Library gets `title=Dune` and
    // `author=Frank+Herbert`, never one phrase searched inside the title.
    let req = build_request("Dune", "Frank Herbert", "9780441013593").expect("has content");
    assert_eq!(req.title.as_deref(), Some("Dune"));
    assert_eq!(req.author.as_deref(), Some("Frank Herbert"));
    assert_eq!(req.isbn.as_deref(), Some("9780441013593"));
}

#[test]
fn build_request_trims_and_treats_a_blank_field_as_absent() {
    let req = build_request("  Dune  ", "   ", "").expect("has content");
    assert_eq!(req.title.as_deref(), Some("Dune"));
    assert_eq!(req.author, None);
    assert_eq!(req.isbn, None);
}

#[test]
fn build_request_keeps_the_structure_however_the_reader_edits_it() {
    // The regression this replaced: a single stray keystroke used to drop all
    // three structured fields and send the whole phrase as free text, which
    // put five books *about* Dune above the novel.
    let req = build_request("Dune ", "Frank Herbert", "").expect("has content");
    assert_eq!(req.title.as_deref(), Some("Dune"));
    assert_eq!(req.author.as_deref(), Some("Frank Herbert"));
}

#[test]
fn build_request_accepts_an_isbn_on_its_own() {
    // The strongest question any provider takes, and one the single box could
    // never express.
    let req = build_request("", "", "9780441013593").expect("an ISBN is enough");
    assert_eq!(req.isbn.as_deref(), Some("9780441013593"));
    assert_eq!(req.title, None);
    assert_eq!(req.query, "", "no title or author to compose from");
}

#[test]
fn build_request_accepts_an_author_on_its_own() {
    let req = build_request("", "Frank Herbert", "").expect("an author is enough");
    assert_eq!(req.author.as_deref(), Some("Frank Herbert"));
    assert_eq!(req.title, None);
}

#[test]
fn build_request_is_none_when_every_field_is_blank() {
    assert!(build_request("", "  ", "").is_none());
}

#[test]
fn build_request_composes_the_free_text_query_for_a_rest_client() {
    assert_eq!(
        build_request("Dune", "Frank Herbert", "")
            .expect("has content")
            .query,
        "Dune Frank Herbert"
    );
    assert_eq!(
        build_request("Dune", "", "").expect("has content").query,
        "Dune"
    );
}
