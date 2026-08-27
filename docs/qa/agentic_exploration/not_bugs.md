# Things that are not bugs

Read this before your first flow. Two kinds of false positive live here:
behaviour the app means to have, and failures of your own equipment that look
like failures of the app. The contract is [start.md](start.md).

When a new false alarm is found and explained, add it here — that is what keeps
the next run from repeating it.

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

## Deliberate app behaviour

Each of these is intended, and each has been mistaken for a defect before:

- **Opening a reader or starting a player changes your read status by itself.**
  Unread becomes reading on open; reaching the end marks finished. You did not
  do that, and it is not a bug.
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
