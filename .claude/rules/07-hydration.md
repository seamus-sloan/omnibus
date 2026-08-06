# 07 — SSR/WASM hydration parity

Omnibus is a Dioxus fullstack app: the server renders HTML (SSR), and the
WASM client re-renders the same component tree and **hydrates** it (adopts
the existing DOM, wiring up event handlers). Hydration assumes the first
client render produces markup identical to the SSR render. When it
doesn't, Dioxus mis-adopts nodes — you get a blank page, a flash of wrong
content, or controls whose handlers never fire.

## The invariant

**Never feature-gate a component *body* on `web` / `mobile` / `server`.**
A component must emit the same rsx on every target; gate only the
*interop* it runs after mount (the `use_effect` that calls
`dioxus::document::eval`, the `gloo_net` fetch). SSR and the first WASM
paint must match.

## Common causes

- **`#[cfg(feature = "web")]` around rsx** — the classic. SSR omits a
  subtree the client renders (or vice versa). Move the gate into the
  effect, not the markup.
- **State that differs SSR vs client at first paint** — e.g. a signal
  seeded from `localStorage` on web but from a default on SSR. Seed both
  to the same value and let an effect reconcile after mount.
- **Non-deterministic content in render** — timestamps, random ids, or
  `Date::now()` baked into markup differ between the two renders.
- **Hook-order divergence** — conditionally declaring hooks (or a
  different count) on one target. Declare every hook unconditionally, in
  the same order, on every target.
- **Swapping element *types* in a conditional** — `if x { img {…} } else {
  span {…} }` makes the diff replace the node rather than update it, and
  handlers registered on siblings rendered alongside it stop firing. The
  symptom is a button that clicks but does nothing, with no console error.
  Keep one stable outer element and swap its *children* instead —
  `components/user_avatar.rs` is the worked example (a journal card's
  Delete died the moment its author had a profile picture).

## Confirming a mismatch

1. Reproduce via the [`ui-validate`](../skills/ui-validate/SKILL.md) skill
   (it drives the real SSR + hydration path), then
   `mcp__Claude_Preview__preview_console_logs` — Dioxus logs hydration
   errors there.
2. Diff the SSR HTML (`curl -s http://127.0.0.1:$OMNIBUS_PORT/<route>`)
   against the hydrated DOM (`preview_snapshot`). A subtree present in one
   but not the other, or in a different order, is the culprit.
3. Find the gate: search the offending component for
   `#[cfg(feature = "web"/"mobile"/"server")]` around rsx, and check any
   signal initialized differently per target.

## Fixing

Pull the cfg gate out of the rsx and into the post-mount effect;
initialize signals to a target-agnostic value and reconcile in an effect.
[frontend/src/view_prefs.rs](../../frontend/src/view_prefs.rs) (SSR
defaults that match first-hydration markup) and the
`frontend/src/components/auth/` primitives ("SSR/WASM identical for
hydration") are the worked examples to mirror.
