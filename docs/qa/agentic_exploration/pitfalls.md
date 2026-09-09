# Testing pitfalls

Read this before your first flow. Two kinds of pitfall live here: behaviour the
app means to have, and failures of your own equipment that look like failures
of the app. Falling into either produces a confident report of a defect that
does not exist, which costs someone a real investigation. The contract is
[start.md](start.md).

When a new pitfall is found and explained, add it here — that is what keeps the
next run from falling into it.

## Trust the DOM, not the pixels

Your browser is equipment, and it fails in ways the app does not. Rule the
equipment out before you report anything you *saw*.

- **A blank, clipped, or collapsed screenshot is usually the pane, not the
  app.** An unfronted pane stops painting and its viewport can collapse to
  nothing. Check `document.hidden` and the window's inner width and height: if
  it is hidden, or far smaller than you expect, front the tab and shoot again.
- **An unfronted pane also swallows input** — clicks land nowhere, scrolls time
  out, and nothing errors. Before concluding a control is broken, confirm the
  pane was fronted when you clicked it.
- **Never read a value off a downscaled screenshot.** Read the text through the
  DOM. A tilde has already been misread as a minus this way, producing a
  confident report of negative reading progress that did not exist.
- **Assert on the DOM and keep screenshots as illustration.** The DOM is the
  app's actual output and does not care whether anything was painted.

A pane that keeps failing is a note about the harness, not a finding about the
app. Say so in the journal, and if it becomes unworkable, stop and report that
rather than pressing on with unreliable evidence.

## Your driver, not the app

- **A command that returns `"Done"` did not fail.** The driver swallows the
  result whenever the command contains a semicolon — **including one inside a
  string literal**: `page.evaluate(() => "a; b")` yields `"Done"` while
  `page.evaluate(() => "a b")` yields `"a b"`. Brace-bodied arrows
  (`async () => { … return x }`) hit it for the same reason. Rewrite without
  semicolons — chain with `.then()` and drop statement separators. `"Done"`
  means rewrite the command, not that the app is broken.
- **A file input cannot be clicked.** Clicking one opens a native dialog nothing
  can see. Upload with
  `page.getByTestId("add-books-file-input").setInputFiles("/abs/path.epub")`
  and then read the review form's fields. A file of any size is fine — a
  multi-part audiobook included.
- **A locator that times out inside a `.then()` chain kills the driver.**
  The rejection is unhandled in the driver's process and Node exits — that is
  what took agent-1's browser down on a Play click. Keep every command one
  top-level `await`, one statement per call, and let a timeout come back as an
  error rather than as a dead server.
- **A mouse drag over the reader's iframe hangs the command** for the full
  timeout while the page stays responsive. Select text with a triple-click,
  or a click and a shift-click; both produce a correct highlight.
- **The scratch directory is shared with every other agent.** A helper script
  you drop there is overwritten by theirs, and one run sent an agent's
  commands into another agent's browser that way. Use a directory named for
  your actor and nothing else.
- **Some names are not what they look like.** The nav's "LIBRARY" is
  uppercase by CSS, so match it case-insensitively. The shelf dialog's Create
  button may read `Create · N` once a name is typed (one agent saw the count,
  another a plain "Create"), and rail chips'
  accessible names are prefixed by kind ("Smart shelf Weeknight Reading").
  A table column sorts from its header's inner **button**, not the cell.
  The journal composer's toolbar buttons are named `Bold`, `Italic`,
  `Strikethrough`, `Heading 1/2`, `Quote`, `Bullet list`, `Numbered list`,
  `Checklist`, `Inline code`, `Link`, `Spoiler — blurred until clicked`.
- **Two surfaces are hover fans.** The continue surface is one (below), and
  so is "From the same hand" on a book page: the author lead card covers all
  but a sliver of each sibling cover until you hover, so a click that lands
  nowhere is the fan, not a dead tile.
- **An open command palette blocks every other click** with its scrim. A
  driver that leaves it open after a failed step then sees each page click
  "intercepted". Close it (Escape) before navigating.
- **A `"driver": "dead"` answer from `driver.sh run` is your harness, not the
  app.** Your browser server died under the command and the app never saw it.
  Journal an anomaly of kind `issue`, run `driver.sh restart <n>`, have the
  guard reinstalled, and repeat the step. `"driver": "up"` on a timeout is the
  opposite case: the server is fine and the command itself never returned —
  an app hang, or a locator that never matched.
- **A `403` with `"error": "ownership_guard"` is your own harness**, not the
  app refusing you. It means a destructive call named a book you did not add.
  Journal it `refused` and move on; retrying or routing around it is the one
  thing the guard exists to prevent.
- **You have your own browser.** If you ever see another actor's session, that
  is a harness fault of the first order — journal it `high` and stop, exactly as
  start.md says. Do not log back in and continue.

## Things that look broken in the DOM and are not

Each of these was nearly filed as a defect by an agent that checked first.

- **`3 wk ago` beside LONGEST SIT is a sparkline axis label**, not part of the
  stat. Flattened `innerText` reads "LONGEST SIT / 9m / Aug 28 / 3 wk ago",
  which looks like today's date being called three weeks old. It lives in
  `.rx-spark-axis`.
- **The mini-player's speed and sleep panels are always in the DOM**, at
  `opacity: 0; pointer-events: none`. They appear in `document.body.innerText`
  on any book page and read as two expanded panels drawn over the page. Check
  computed style before believing a panel is open.
- **The persistent mini-player lives outside `<main>`.** Audio playing with "no
  visible transport" usually means you only looked inside `main` — and the
  listen page has no `<main>` landmark at all, so read `document.body` there.
- **The journal composer is a CodeMirror contenteditable.** `locator.fill()`
  silently strips every newline, collapsing a multi-paragraph entry to one line
  — which looks exactly like the app truncating your text. Use
  `keyboard.type`. Neither `End` nor `ControlOrMeta+End` moves the caret to
  the end there, so an "append" can land mid-document, and an empty line is a
  zero-size span the driver cannot click — reach it with ArrowDown from a
  neighbour.
- **"Synced here" in the reader footer is a button**, not a status. It
  declares the ebook and audiobook aligned at that spot, and it is shown on
  ebook-only books too. It never jumps anywhere.

## Deliberate app behaviour

Each of these is intended, and each has been mistaken for a defect before:

- **Opening a reader or starting a player changes your read status by itself —
  on the web.** Unread becomes reading on open; reaching the end marks
  finished. You did not do that, and it is not a bug. **It holds on the iOS
  native reader too** — #2289 is fixed, and two agents in run r-20260908-02
  watched the transition happen and survive a relaunch.
- **Read status filters the continue surface** on the home page. A book you
  just marked finished vanishing from it is correct.
- **The continue surface is an overlapping fan**, not a carousel — cards sit on
  top of one another until you hover.
- **Book covers are not links** but list items. Clicking works; middle-click and
  "open in new tab" may not.
- **Shelf pages are a mobile surface.** On the web, picking a shelf filters the
  library in place and the URL does not change. Only the iOS agent gets a
  dedicated shelf screen.
- **Other agents are working in the same library at the same time.** Covers
  changing, books appearing, a title you were looking at getting edited — that
  is another reader, not corruption. Only call it a finding if *your own* data
  changed underneath you.
- **Indexing is asynchronous.** A newly added book may take a moment to appear.
  Wait and re-check before reporting it missing.
- **The library reloads through its defaults.** For two or three seconds
  after a reload the page shows the grid toggle, Title sort, no count and no
  books; then the saved view, sort and direction come back. A read taken in
  that window sees a revert that is not one. The book detail page likewise
  shows a bare "Loading…" shell for several seconds on reload.
- **Remaining time in the player is rate-adjusted.** At 1.5× elapsed is book
  time and remaining is wall time, so the two do not sum to the total. That
  is the intended clock (#2246), not a counter fault.
- **Every reader's wishlist shelf is listed for everyone, marked Public.**
  Seeing other readers' wishlists on the rail or the iOS Shelves grid is by
  design; seeing their *entries inside your own* wishlist is the finding.
- **PHYS is a table-view badge.** On the web a paper copy shows in the table's
  Formats cell and on the detail page's Physical copy card; grid tiles carry
  no badge on either surface.
- **Re-running a check-in lookup on a book you already filed navigates
  straight to it** on the web, with no message; iOS shows an "Already on your
  shelf" card instead. Both are recognition, not silence.
- **The book detail page has two layouts, chosen by a per-user setting.** With
  scroll stops off (the default) it is one continuous page; with them on it
  snaps section by section, with a dot rail down the side. Only one exists at
  a time, and which one you see follows your own account page toggle — another
  agent seeing the other layout is not a disagreement.
- **The stats window pills move one band only.** Week / Month / Year /
  Lifetime govern the "In this window" band; the streak, the goals, the
  heatmap and the open-books list are standing figures and are *meant* to stay
  put. Under Lifetime the per-tile comparisons are absent by design.
- **Tag and genre chips on a detail page are not links.** They are inert on the
  page, and editable only through the `+` control beside them. Clicking one
  and going nowhere is correct.

## Driving the surfaces — added after run r-20260908-02

- **Shift-click does not extend a selection in the epub iframe.** The earlier
  guidance here said a click plus a shift-click produces a correct highlight;
  three agents found it either leaves the selection unchanged or empties it.
  **Triple-click on a paragraph is the only reliable way to select text**, and
  a drag still hangs the driver.
- **The reader lays a whole spine item out in one iframe about 74,000 px wide,
  shifted left.** Any `getBoundingClientRect` or `caretRangeFromPoint` reading
  must add the iframe's own `left` offset, or it reports the front matter
  wherever you actually are. One agent nearly filed "page turns don't advance
  the content" off that before checking the geometry.
- **Expanding a journal entry covers only one column.** `read →` opens
  `.bdmq-overlay` — fixed, `z-index: 8`, a 55% black backdrop — over the
  right-hand content column and *not* the viewport. Clicks on Export and the
  read-status buttons then land on the backdrop and do nothing, while a control
  in the strip above the overlay still works, which reads as randomly dead
  controls. Close the entry with **✕ Close** before touching anything else.
- **The dot rail does not tell the two book-detail layouts apart.**
  `bdmq-dot-0…5` render in both; the container does — `#bdmq-flow` with
  `.bdmq-flowlab` rules for the continuous layout, `#bdmq-snap` with
  `.bdmq-sec` screens for the snapped one. Rule 04a already said so.
- **The shelf name input has two different testids** — `shelf-name-input` on
  the create form, `edit-shelf-name` on the edit form — and the Kobo opt-in is
  a segmented pair of buttons (`edit-shelf-kobo-off` / `-on`) carrying
  `aria-pressed`, not a checkbox. The New-shelf and Edit-shelf forms are a
  modal overlay with **no** `role="dialog"` and no accessible name, so
  `getByRole("dialog")` matches nothing.
- **A series card carries two overlapping anchors** with the same href and the
  same accessible name, so `getByRole("link", {name}).first().click()` times
  out with a pointer-interception error. A person clicking the card navigates
  fine.
- **On iOS, a SwiftUI switch ignores a zero-duration tap.** Use a drag, or a
  tap with `duration: 0.2`. Buttons, segmented controls, star ratings and menu
  items all answer a zero-duration tap normally.
- **On iOS, the `text` action drops non-ASCII.** To type an accented query, put
  it on the pasteboard with `xcrun simctl pbcopy` and long-press → Paste —
  and note that `simctl pbcopy` decodes its input as MacRoman, so the bytes
  need pre-encoding.
- **An identifier row labelled ISBN-10 holding a 13-digit value is the file's
  own claim**, not a rendering bug: the row's label comes from the OPF scheme.
  Two runs have now reported it. What *is* a defect is that a saved ISBN
  override never reaches that table at all (#2496).
