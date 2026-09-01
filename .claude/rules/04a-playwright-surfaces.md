# 04a — Playwright per-surface selectors

Companion to [04-playwright.md](04-playwright.md), which owns the toolchain
and the general conventions. This file is the list of surfaces whose obvious
selector is the wrong one — each entry exists because a spec was written
against markup that had changed, and timed out.

**A book tile is not a `link`.** The landing grid tile (`landing/grid.rs`) is an
`<a>` with **no `href`** and an explicit `role="listitem"`, and the shelf member
tile (`components/cover_tile.rs`) is a router `Link` that also overrides
`role="listitem"` — so `getByRole("link", { name: "Open details for …" })`
matches nothing and times out. Use `bookTile()` from `utils/shelves.ts`
(`getByRole("listitem", …)`) or the per-book `ebook-tile-<ident>` testid.

**The book detail panel reads two ways.** `book_detail_scroll_stops` (off by
default) chooses between the flow — one continuous scroller, `#bdmq-flow`,
sections introduced by `.bdmq-flowlab` rules, a `bdmq-flowtop` back-to-the-book
pill, and no `.bdmq-next` cue — and the snapped marquee, `#bdmq-snap` with
`.bdmq-sec` screens. Only one exists in the DOM at a time, so a spec asserting
either must set the preference first
(`POST /api/account/book-detail-scroll-stops`) and put it back afterwards; the
dot rail (`bdmq-dots`, six rows) is the one control common to both.

**A landing *table* row is not a `button`.** The row (`landing/table/row.rs`)
used to be `<tr role="button">` with an "Open details for …" name, but it
wraps interactive editable cells (invalid ARIA), so that was dropped (#2350).
The keyboard-reachable navigate affordance is now a `link` on the cover cell:
`getByRole("link", { name: "Open details for …" })` **inside the row**, or the
`ebook-cell-cover` testid — not a row-level button, and clicking an editable
cell (title/author/…) still opens its inline editor rather than navigating.

**The landing's continue surface is a fan, not a carousel.** `landing/stack.rs`
replaced the hero carousel, so `continue-hero`, `continue-hero-track`,
`hero-dot-<n>` and `hero-card-<uuid>` no longer exist. The section is
`continue-stack`; **every** card in the fan is `hero-resume-<uuid>`, whichever
position it holds, and the front one additionally carries the `lead` class and
the `.lmq-veil` verb. Three consequences for a spec:

- **A card's centre is not its own.** The fan overlaps by ~74px at rest, so
  `click()` on a card behind the front one hits its neighbour. Hover
  `.lmq-fan` first (which spreads it) and click the left sliver —
  `click({ position: { x: 12, y: 60 } })`.
- **The cross-format chips describe the front book only.** `hero-immersive-*`,
  `hero-crossformat-*` and `hero-link-invite-*` render for the lead card alone,
  so a spec that asserts them must bring its book forward first — clicking a
  card that is *already* lead navigates, so guard on the `lead` class.
- **`continue-stack` carries the synced marker**, not the card: the stack has
  no per-card eyebrow, so `Continue · synced` became one kicker line above the
  front book.

`standalone-island` / `standalone-garden` are reserved for the bring-forward
spec on the usual grounds: it needs two books on the stack at once, and
`db::progress::recent_progress` drops `unread` and `finished`, so any book
another spec sets read status on could be filtered off the fan mid-run.

**The stats period pills are not a menu, and half the page is not behind
them.** `/stats` splits on the windowed / standing boundary: the Week / Month /
Year / Lifetime pills (`stats-range-week` … `stats-range-all`) live in the
"In this window" band's own sticky header and govern that band alone. There is
no `role="dialog"` period menu and no `stats-range-trigger` — a spec that opens
one is asserting a control that no longer exists. Three consequences:

- **The hero and the standing band must not move on a switch.** The streak,
  both goals, the heatmap, the open-books list and the trailing-12 chart are
  standing figures; `stats.spec.ts` pins that with a before/after `textContent`
  comparison, and that test is the redesign's load-bearing assertion.
- **The library figures are behind a scope tab**, not below the fold —
  `stats-scope-tab-library` has to be clicked before `stats-library-size` or
  `stats-library-composition` exist in the DOM at all.
- **Lifetime drops every tile comparison.** `PeriodComparison` is `Default` for
  `AllTime`, so a delta drawn against it would report a reader's whole history
  as new; `stats-tile-*-delta` is absent there by design, not missing.
- **The page reports goals but never edits them.** All three are set together
  in Settings → Account, so the goal-editing specs live in `account.spec.ts`
  (serial — they reset the same three per-user rows). `stats.spec.ts` asserts
  only that the editors stay gone and that the read-only states render.
- **An unset goal is not an empty state.** With no target the surface drops the
  ring and the bar but still reports the real figure — `stats-goal-today`,
  `stats-daily-{kind}-today` — so a spec asserting "no goal" must look for
  those, not for an invite. `*-invite` now renders only when the server sent no
  figure at all.
