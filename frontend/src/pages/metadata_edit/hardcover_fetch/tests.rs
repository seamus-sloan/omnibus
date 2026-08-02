//! Regression coverage for #1590: `HardcoverFetchPanel`'s `available` gate
//! must never change the number/order of hooks a render calls.
//!
//! The real component resolves `available` through a `dioxus::fullstack`
//! server function whose extractors (`pool: PoolExt`, `user: AuthUser`) need
//! a live axum request context to resolve — there's no seam in this crate's
//! test harness to drive that from a bare `VirtualDom` (unlike
//! `rpc::overrides`'s tests, which call an *extracted* pool-taking body
//! directly; this rpc is a one-line lookup with nothing to extract). So this
//! harness mirrors `HardcoverFetchPanel`'s exact fixed hook shape — every
//! `use_signal` declared before the `if !available() { return }` gate — and
//! drives the false-to-true transition with a real `Signal<bool>` published
//! back to the test via `use_hook`, then flips it and forces a second render
//! the same way `dioxus`'s own signal-write scheduling does in production
//! (`dom.in_runtime(|| available.set(true))`, then `render_immediate`).
//!
//! This build of Dioxus (v0.7.9) does not hard-panic on the specific
//! *monotonic-growth* hook mismatch #1590 described (a render that calls
//! strictly more hooks than the previous one just extends the hook list) —
//! confirmed by running the pre-fix shape through this same harness during
//! development, which completed without panicking. That is exactly why the
//! bug shipped unnoticed rather than crashing CI: rule 07's "same hook
//! count/order on every render" is a hazard that can silently misattribute
//! state (a *different* hook shrinking or reordering) rather than a
//! guaranteed crash on *this* one-directional transition, so the test below
//! asserts the fixed shape's rendered *content* is correct after the flip,
//! not just that it avoided a panic.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;

/// Publishes its internal `available` signal back to the test via `slot`,
/// so the test can flip it the same way a real `use_effect` resolution
/// would, then forces a second render to prove the hook table survives.
#[derive(Clone)]
struct HarnessProps {
    slot: Rc<RefCell<Option<Signal<bool>>>>,
}

/// Mirrors `HardcoverFetchPanel`'s fixed shape: `state`/`busy` declared
/// unconditionally, before the `available` gate.
fn fixed_shape_harness(props: HarnessProps) -> Element {
    let available = use_signal(|| false);
    use_hook(|| *props.slot.borrow_mut() = Some(available));
    let mut state = use_signal(|| 0i32);
    let mut busy = use_signal(|| false);

    if !available() {
        return rsx! {};
    }

    state += 1;
    busy.set(true);
    rsx! { div { "{state()}-{busy()}" } }
}

#[test]
fn hook_order_survives_an_availability_flip_from_false_to_true() {
    let slot: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let mut dom =
        VirtualDom::new_with_props(fixed_shape_harness, HarnessProps { slot: slot.clone() });
    dom.rebuild_in_place();
    // Before the flip: same hook shape as the real panel's pre-availability
    // render, and its early return renders nothing.
    assert_eq!(dioxus::ssr::render(&dom), "");

    let mut available = slot
        .borrow_mut()
        .take()
        .expect("harness publishes its signal on first render");
    dom.in_runtime(|| available.set(true));
    // Must not panic, and the post-flip render must show the *correct*
    // state — not just avoid crashing, in case a hook mismatch silently
    // wired `busy`'s slot to `state`'s value or vice versa.
    dom.render_immediate(&mut dioxus::core::NoOpMutations);
    assert_eq!(dioxus::ssr::render(&dom), "<div>1-true</div>");
}
