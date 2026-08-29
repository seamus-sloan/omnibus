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
  and then read the review form's fields.
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
  visible transport" usually means you only looked inside `main`.
- **The journal composer is a CodeMirror contenteditable.** `locator.fill()`
  silently strips every newline, collapsing a multi-paragraph entry to one line
  — which looks exactly like the app truncating your text. Use
  `keyboard.type`. `ControlOrMeta+End` also does not move the caret to the end
  there, so an "append" can land mid-document.

## Deliberate app behaviour

Each of these is intended, and each has been mistaken for a defect before:

- **Opening a reader or starting a player changes your read status by itself —
  on the web.** Unread becomes reading on open; reaching the end marks
  finished. You did not do that, and it is not a bug. **This does not currently
  hold on the iOS native reader** (#2289): a book read there comes back with no
  read status at all, while the Library's continue card still shows it as
  Reading, so the two surfaces disagree. On iOS, treat an unchanged status as
  that known bug rather than a new finding — and do not rely on the transition
  to put a book into a status you need, because it will not.
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
