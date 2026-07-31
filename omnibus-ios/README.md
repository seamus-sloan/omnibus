# Omnibus for iOS (native)

A native SwiftUI client for a self-hosted Omnibus server — an experiment in
replacing the Dioxus/WKWebView shell in [`mobile/`](../mobile) with something
that feels like an iOS app. The Rust mobile crate is untouched and still builds;
this is a parallel client, not a replacement.

## Why it exists

The hybrid build renders shared `omnibus-frontend` rsx into a system WebView.
That gives one codebase for web + iOS + Android, but it can't give iOS scroll
physics, interactive swipe-back, native navigation transitions, keyboard
avoidance, or lock-screen audio. This app talks to the same server over the same
`/api/*` REST surface and gets all of that from the platform.

## Running it

Quickest path, from the repo root (system xcodebuild + simctl — no nix shell):

```bash
just ios-sim       # boot the newest iPhone simulator, build, install, launch
just ios-build     # compile check only (generic simulator destination)
just ios-test      # omnibusTests — the same scripts/ios-test.sh invocation CI runs
just ios-test-ui   # omnibusUITests
```

`ios-sim` builds into `~/.cache/omnibus-ios-derived/<worktree>` (build artifacts
stay out of the tree, like `CARGO_TARGET_DIR`) and, when `just dev-up` has run,
prints this workspace's dev-server URL to enter on the Connect screen. Pin a
simulator with `OMNIBUS_IOS_SIM_UDID`; test results land as
`.claude/runtime/ios-tests/<suite>.xcresult`.

Or through Xcode:

1. Start a server the simulator can reach, pointed at the full fixture set:

```bash
EBOOK_LIBRARY_PATH=$PWD/test_data/epubs AUDIOBOOK_LIBRARY_PATH=$PWD/test_data/audiobooks cargo run -p omnibus
```

   Pointing at `test_data/epubs` (rather than a subfolder) picks up both the
   generated fixtures and the public-domain set, which is what gives the grid
   real cover art and per-book accent colors to design against.

2. Open `omnibus.xcodeproj`, run on any iOS 26 simulator.
3. On the Connect screen enter the host — `127.0.0.1:3000` from the simulator,
   or your Mac's LAN IP from a device — then sign in.

`http://` is allowed via an ATS exception in [`Info.plist`](Info.plist): a
self-hosted app points at arbitrary LAN addresses, so there is no fixed domain
to scope an exception to.

## Layout

| Path | What's in it |
|---|---|
| `omnibus/App/` | `AppState` (auth phase, server URL, theme), root routing, tab shell |
| `omnibus/Design/` | Atrium palette ported from `frontend/assets/atrium.css`, shared components |
| `omnibus/Models/` | `Codable` mirrors of `omnibus-shared` wire types |
| `omnibus/Networking/` | Bearer-auth `APIClient`, Keychain token store |
| `omnibus/Offline/` | SQLite replica cache, mutation outbox, download manager, reachability |
| `omnibus/Services/` | Read/write facades over the API + cache |
| `omnibus/Features/` | One folder per screen |
| `omnibus/Reader/` | EPUB reader: native chrome over the vendored epub.js engine |

## Shell

Four tabs, one job each — **Library · Search · Stats · You**.

The landing screen answers one question: *what do I want to do right now?* For a
reading app that is almost always "carry on with what I'm reading", so it leads
with a **Continue hero** — a full-width card with real artwork, progress, and a
Play/Read button. More than one book in progress pages horizontally in the same
card's worth of space rather than costing another band of the screen.

Below it, the two things worth showing off: **shelves as cover mosaics**, then
the **collection** itself as a grid. Nothing else. Sort and format collapse into
one toolbar menu.

Everything that used to crowd the landing moved to the tab that owns it:

| Was | Now |
|---|---|
| Search pill in a pinned header | The Search tab |
| Add (+) in the header | You → Add books |
| Category strip (All/Ebooks/Audiobooks/Offline) | The one toolbar menu, beside sort |
| Authors tab | A browse row in Search (and in You) |
| Series / Tags browse pills | Browse rows in Search |

Search owns *finding things*, so browsing by author, series, shelf, or tag lives
under the field rather than on the landing — and, while the field is empty, a
**Recently finished** rail, which is the other thing readers like to show off.

- **A flush bottom bar** — full-width, hairline rule, outline icons that fill on
  selection. `TabView` still owns tab state and lazy loading; only its chrome is
  replaced (`.toolbar(.hidden, for: .tabBar)` plus a `safeAreaInset`), so scroll
  position and in-flight loads survive switching.
- **The mini player stacks directly above the tabs** as one block, with a
  hairline of playback progress along its top edge.

> Backgrounds on both bars use `.background(_:ignoresSafeAreaEdges:)`. Putting
> `.ignoresSafeArea()` on the background *view* instead extends the fill but
> drags the labels down into the home indicator.

## Shelves

A shelf's identity is the books on it, so shelves are drawn as their own
contents rather than as a name on a coloured square:

- **A shelf is a mosaic.** Four 2:3 covers in a 2×2 grid come out 2:3
  themselves, so a shelf tile drops into the same grid rhythm as a book with no
  special case. One book fills the tile; empty slots take a tone derived from
  the shelf's accent, or its name when it has none. `GET /api/shelves` carries
  only a count, so the artwork needs one page read per shelf — they run
  concurrently and the whole set is cached as a unit.
- **They live on the landing screen.** Behind a browse pill they were
  effectively invisible; as a rail under Continue they read as part of the
  library, with a dashed "New shelf" tile at the end so creating one doesn't
  require finding the index first.
- **Adding a book is a toggle, not a one-shot.** The picker lists every manual
  shelf with its mosaic and a live checkmark, so you can see what a book is
  already on and take it off again. Smart shelves appear read-only under
  "Automatic" — their membership is derived, and omitting them entirely read as
  a bug. Book detail shows the state on the button itself ("Add to shelf" /
  "On 2 shelves") rather than burying the action in the overflow menu.
- **Smart shelf conditions are visible** on the shelf's own screen, so you
  don't have to open an editor to find out why a book is or isn't on it.

## Notes on a few decisions

**Cover art drives the color.** The indexer extracts a dominant `accent` per
book and the API hands it over on every row. It sets the wash behind the
book-detail hero and tints the Continue cards, so screens take their color from
the book rather than from one global accent. Books with no cover art get a
generated plate — an accent gradient with the title set in the display face,
falling back to a title-derived hue — which is what keeps a shelf of coverless
books from reading as grey noise. `CoverIdentity.tone` is the single place that
resolves it, so the plate, the hero wash, and the Continue card can't disagree
and show a purple book under an orange wash. Every cover renders into the same 2:3 box and
fills it, so a grid row can't go ragged when source images disagree on aspect
ratio.

**The palette is OKLCH, not hex.** `Design/OKLCH.swift` implements the
OKLCH→sRGB conversion so tokens can be written in the same notation as
`atrium.css`. A change on either side is a direct comparison instead of a
guess.

**Serde omits things Swift's decoder requires.** The server drops `false` bools
and empty arrays (`skip_serializing_if`). Swift's synthesized `Decodable`
ignores property defaults and hard-fails on a missing key, so `Models.swift`
adds `KeyedDecodingContainer` overloads for `Bool` and arrays. Deliberately not
for `String`/`Int` — defaulting those would hide a real contract break.

**The EPUB reader reuses epub.js.** iOS ships no EPUB renderer, and the
vendored glue already implements CFI positions, pagination, and annotations
against exactly the contract the server expects. `Reader/Web/` is a copy of
`frontend/assets/vendor/`; everything around it — chrome, tap zones, sheets,
gestures, persistence — is native. Assets and the book are served over a custom
`omnibus-reader://` scheme so epub.js sees one same-origin space and needs no
cookie.

**The glue owns gestures, not SwiftUI.** `epub-reader-glue.js` already
implements swipe-to-turn (the page tracks your finger, with velocity and
selection gating), outer-20%-gutter taps, and a centre-tap chrome toggle that
it signals through `__omnibusOnToggleChrome`. A SwiftUI tap-zone overlay above
the web view is hit-testable across the whole screen, so it swallows every
touch those depend on — page turns still worked because the overlay called
`next()` itself, which masked the fact that swipe was dead. The host now layers
nothing interactive over the page.

**Reading chrome follows Apple Books.** A book opens bare — no buttons at all,
just the page between two centred labels. A centre tap brings up a `✕`
top-right and one menu button bottom-right; another puts them away.

The labels never leave, but they answer a different question in each state,
because the two states are different activities (`ReaderIndicators`). Reading,
they name where you are and nothing else: the **book's title** above, the bare
**page number** below. With the chrome up you are navigating rather than
reading, so they widen into "**N pages left in chapter**" and "**361 of 661**"
— "pages left in chapter" being the question you actually have mid-book, which
a raw page number can't answer. Both fall back while epub.js's whole-book
locations pass is still running: the top line to "Chapter N of M", the bottom
to a percentage, since a page number of 0 would be both wrong and jumpy.

Everything else is behind the one menu, as rows: **Contents · N%**, **Bookmarks
& Highlights** (with a count), **Themes & Settings**, then a strip of round
quick actions. Contents, bookmarks, and highlights share one segmented sheet,
since all three answer "take me somewhere in this book".

The **Contents row doubles as the scrubber** — press and hold, then slide. That
is where Books hides seeking, and it's why the row carries a percentage at all.
A plain tap on the same row opens the contents; one `DragGesture` serves both
verbs so a tap and a hold can't fight over the touch. The seek commits on
release (epub.js re-paginates on every `display`, so seeking each intermediate
position stutters and overshoots), and until then the row becomes a track and a
readout above it names the chapter and page you'd land on — one row width
covers the whole book, so without that a slide is unaimable. Jumping leaves a
round return button top-left holding the page you left, opposite the close
button: it has to persist to be useful, and anything wider would be sitting on
the prose.

The chapter/page readout needs to know where each contents entry *starts*, which
epub.js can only answer once its locations pass has run. So the toc is emitted
twice — once bare, so the contents list works immediately, then again with a
`page` and `pct` per entry. Those same numbers are the page column in the
contents list.

All of it sits in bands `#stage` reserves via `env(safe-area-inset-*)`, so
prose never runs under the notch, the home indicator, or the chrome itself.
Those bands are sized to clear the floating controls: the chrome is laid *over*
the page, and re-paginating every time the bars toggle would cost a reflow per
tap.

> `.glassEffect` goes **under** a button, never around it. Wrapped around one
> (especially as `.interactive()`) the glass takes the touch and the button's
> action never runs — it looks and animates fine, so this reads as a dead
> control with no error anywhere. Given one glass circle per button, neighbours
> a few points apart also contend with each other and the *middle* one goes
> dead while the outer two keep working; a `GlassEffectContainer` does not fix
> that. Put the effect on the label, inside the button — `ReaderGlassButton`
> and `ReaderMenuRow` in `Reader/ReaderChrome.swift` are the shape to copy.

> Glass resolves against the environment's colour scheme, not the pixels behind
> it, so reader chrome takes `.environment(\.colorScheme,)` from the *reading*
> theme — otherwise app-dark chrome lands as a grey slab on a white page. The
> presented sheets opt back out to the app's scheme; they're app surfaces, not
> page surfaces.

> Two things the stage markup has to keep: `viewport-fit=cover` in the viewport
> meta (or `env()` resolves to zero), and `class="rd-stage"` on the mount node —
> the glue's host-level gesture handler tests for it, and without it swipes
> landing on the stage margins rather than inside the section iframe are ignored.

> `setMargins` assigns its argument straight to CSS `max-width`, so it needs a
> CSS length (`"80%"`), not a bare number. A number produces `max-width: 46`,
> which is invalid and silently dropped — the symptom is prose running to the
> screen edges with the margin control appearing to do nothing.

> `Reader/Web/epub-reader-glue.js` has **diverged** from
> `frontend/assets/vendor/` — it carries iOS-only additions the web build
> doesn't have: annotation taps (`__omnibusOnAnnotationTap`), note underlines,
> `seek(fraction)` for the scrubber, `chapterPagesLeft` on the relocate payload,
> `applyHostGround` (paints the host document the page's ground, so the bands
> around `#stage` don't stay dark behind a light page), and `clearSelection`
> (the selection belongs to the section iframe, so the host window's
> `getSelection` can't drop it). Merge changes from
> the web copy rather than overwriting. The bridge depends on
> `buildRelocateData`'s payload shape (`pct`, `chapter` as an index,
> `chapterTitle`, `chapterPagesLeft`).

**One tap on a highlight arrives twice.** The mark's own listener reports an
annotation tap, and the same touch bubbles to the document as a page tap — and
the mark wins, because a listener on the element runs in the target phase
before the document's runs in the bubble phase. Left unhandled the bars flip
underneath the menu as it opens, and closing the menu then takes a second tap
to put them back. `ReaderController` pairs the two by arrival time (a page tap
within 300ms of an annotation tap is the same gesture) rather than trying to
identify the touch target: which element actually receives it depends on how
epub.js's mark pane handles pointer events, and the tap-target check does not
hold. While the menu is open a transparent SwiftUI layer takes every touch, so
one tap anywhere closes it and a gutter tap can't turn the page out from under
a menu that is about a passage on it.

**ATS is off, and `NSAllowsArbitraryLoads` has to be the only key.** Omnibus is
self-hosted, so the server is whatever machine the reader runs it on — usually
plain http on a LAN address or a private hostname, with no fixed domain to pin
an ATS exception to. iOS 10 and later *ignore* `NSAllowsArbitraryLoads` whenever
`NSAllowsLocalNetworking` (or the ForMedia / InWebContent variants) sits beside
it, falling back to those narrower keys and silently re-enforcing ATS for every
address that isn't link-local. The symptom is a remote server over http failing
with "the resource could not be loaded because the App Transport Security
policy requires the use of a secure connection" while a LAN server keeps
working — so it hides from exactly the setup most likely to be used while
developing. Arbitrary loads already covers local networking; don't add the
narrower key back. (`NSLocalNetworkUsageDescription` is unrelated — that's the
iOS 14+ local-network permission prompt, and it stays.)

**Audio is AVPlayer, not hls.js.** AVPlayer speaks HLS natively, so the
segmented-transcode fallback needs no JS. Multi-part direct manifests are
stitched into one `AVMutableComposition` so the timeline and seeking stay
continuous. `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter` give lock-screen,
Control Center, AirPlay, and CarPlay transport.

**The chapter is the unit of the player, Audible-style.** The layout follows
Audible's, top to bottom: artwork, then the chapter name left-aligned behind a
`☰` and tappable straight into the chapter list, then the scrubber, then a
five-slot transport (chapter back · skip back · play · skip forward · chapter
forward), then Speed · Car Mode · Sleep · Bookmark.

The scrubber spans the *current chapter*, not the book: on a twelve-hour
audiobook a whole-book bar puts every chapter boundary within a couple of points
of its neighbours, so the one gesture the control exists for can't be aimed. The
three readouts under it are chapter elapsed, **book** remaining, chapter
remaining — all at once, which is what pays for the chapter-scoped bar without
needing a mode. The mini bar's hairline stays whole-book for the same reason.

It's a hand-rolled `PlayerScrubber`, not a `Slider`: `Slider` draws a fixed 27pt
pill thumb and its own inset chrome, and no amount of tinting gets it to the slim
track every player of this shape uses. The parent owns the drag position so the
thumb and the readouts can't disagree; a plain tap on the track seeks there too.

The play disc is intrinsically sized rather than taking an equal fifth of the
transport, so the four steppers split what's left and it can't be squeezed —
which is what went wrong when chapter-skip first moved into this row.

`ChapterTimeline` owns every bit of the arithmetic — which chapter a position is
in, how long it runs, where its span starts — as a value type with no AVPlayer
behind it, because the scrubber, the "Ch N of M" readout, the countdown and the
prev/next enablement all read the same three functions, so an off-by-one surfaces
in four places and in none of them obviously. Two cases it exists to pin: a
chapter that ships `duration_seconds` as 0 (common in real files) measures to the
next chapter's start, or to the end of the book if it's the last; and a position
*before* the first chapter's mark resolves to that chapter rather than to `nil`,
since containers routinely start chapter one a second or two in and "no chapter"
is not a place you can be listening. A book with no marks at all degrades to a
whole-book span, and the chapter row and lock-screen chapter commands drop out.

**The artwork gets a share of the screen, not the remainder.** Sizing the cover
as whatever the transport left over made it resize between books — a title that
wrapped to two lines cost it 30pt — and on a short phone it drove the layout's
spacers to zero, wedging the cover against the scrubber. It now takes a fraction
of the band between the two safe-area insets. The fraction tapers on a short
screen (0.66 under a 380pt band, 0.76 above): the transport's height is fixed, so
on a 4.7" phone it claims a far larger share of the screen than on a 6.3" one, and
holding the tall-screen fraction there re-creates the squeeze.

> **An aspect-fill image in a layout container grows it.** `RemoteImage` renders
> `.resizable().scaledToFill()`, and a `.fill` aspect ratio *reports* a size
> larger than the box it fills — that is what filling means. Sat directly in the
> player's backdrop `ZStack` with no frame and no `.clipped()`, it grew the stack
> past the screen, and `safeAreaInset` then measured the chrome against those
> bounds: the top bar landed under the status bar and the utility row's labels
> fell off the bottom. Nothing errors and nothing logs.
>
> It only reproduces on a book with **real cover art** — a generated plate is
> gradients and `Color`s, all infinitely flexible, so they accept the proposal
> instead of overriding it. Every generated audiobook fixture is coverless, which
> is how this shipped: put a cover on one (`POST /api/ebooks/{uuid}/cover`)
> before trusting a player screenshot. Hold decorative art in a
> `Color.clear.overlay { … }.clipped()` — an overlay can't grow its parent, which
> is the same reason `BookCover` puts its art in one.

**Car Mode is a face, not a mode.** It drives the same `AudioPlayer` and adds no
playback behaviour — five targets none smaller than a thumb, and nothing that
needs aiming (scrubber, chapter list, speed, bookmarks) reachable from it. It
holds `isIdleTimerDisabled` while open, since a driver won't tap every 30 seconds
to keep the screen up and a dark screen is the only reason they'd look down.

**Cached reads have a freshness bound.** `Cache.readThrough` serves the replica
within `freshnessWindow` (45s) and goes to the network past it, falling back to
the replica either way when offline. Without the bound, anything changed
elsewhere — another device, the web client, a script — stayed invisible
indefinitely: the background refresh updated the replica but the caller already
had the old value and nothing published the update. Resume points are stricter
still (network-first), because a stale empty list hides the Continue rail
entirely.

**Pull-to-refresh uses `refreshTask`, never `refreshable`.** SwiftUI cancels a
`refreshable` action the moment the view it is attached to invalidates, and
every refresh here publishes as it goes — the replica first, the server's answer
when it lands. Publishing the replica redrew the view, which cancelled the
action, which cancelled the request still in flight behind it: a pull re-rendered
the same stale page, so a book added or removed on the server never showed up
until the app was relaunched. Changing a filter did pick it up, because that
reload runs in a task of its own — which is what made this look like a caching
bug rather than a cancellation one. `View.refreshTask` runs the work
unstructured and awaits it, detaching it from that cancellation while keeping
the spinner up until the refresh has settled.

**Offline is Swift-native.** The Rust layer pairs its download registry with a
loopback HTTP server so the WebView can play a local file. `AVPlayer` and
`WKWebView` both load `file://` directly, so that hop is gone. The rest is a
port: a SQLite kv replica, a coalescing mutation outbox that drains on
reconnect, and per-account scoping that wipes user-scoped keys on a user
switch.

**Tokens live in the Keychain.** The Rust shell persists a 0o600 file and
carries a TODO to harden it; this does that.

## Known gaps

- Android is not covered. `mobile/` builds for both from one codebase; this is
  iOS-only.
- Journal entries render as raw markdown source rather than the server's
  sanitized `body_html`.
- Only the dark themes have been exercised end to end; Light and Sepia are
  wired through the same tokens but unverified on every screen.
- "Recently finished" is sourced from `stats.finished_books`, which the server
  derives from journal entries at 100% progress — marking a book Finished via
  read status alone does not populate it. The server also caches stats for 60s,
  so a fresh entry takes up to a minute to appear.
- Author photo editing, book merging, and the admin log viewer are not ported.
- Test coverage is limited to the pure logic worth pinning — the offline layer
  (`omnibusTests/OfflineSyncTests.swift`) and the player's chapter arithmetic
  (`omnibusTests/ChapterTimelineTests.swift`). No screen-level coverage.
