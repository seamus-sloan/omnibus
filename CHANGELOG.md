# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Releases are cut automatically on merge to `main` (see
`.github/pull_request_template.md`): by default a merge cuts a patch
release, and the `minor version` / `patch version` / `no release`
labels override that default. Unlabeled PRs that only touch docs
(`*.md`, `docs/`) or CI (`.github/`) skip the release automatically.
Entries here are curated manually rather than generated from the raw
commit log.

## [Unreleased]

### Fixed

- iOS app: a self-hosted server reachable only over plain http could not be
  connected to at all — entering its address failed with "the App Transport
  Security policy requires the use of a secure connection". The Info.plist set
  `NSAllowsArbitraryLoads` alongside `NSAllowsLocalNetworking`, and iOS 10 and
  later *ignore* the former whenever the latter is present, so ATS stayed fully
  enforced for every address that wasn't link-local. A server on the same LAN
  worked, which is what hid it
- iOS reader: the highlight menu is no longer fiddly. Tapping a highlight also
  registered as a tap on the page, so the bars flipped underneath the menu as
  it opened and closing it left them wrong until another tap put them back;
  and highlighting a passage left it visibly selected, which spent the next tap
  dismissing the selection instead of doing what you asked. One tap anywhere
  now closes the menu — including in the page gutters, which used to turn the
  page out from under it

### Added

- iOS reader: Apple Books-style chrome. The reading view is now a close button,
  a menu button, and two centred labels — "N pages left in chapter" and the
  page count — instead of filled bars framing the page. Everything else sits
  behind the one menu: Contents, Bookmarks & Highlights, Themes & Settings, and
  a strip of quick actions. The Contents row doubles as a scrubber — press and
  hold, then slide, with the chapter and page you'd land on named above it — so
  seeking to any point in a book no longer means opening the contents and
  picking a chapter, and a jump leaves a return button holding the page you
  left. Contents, bookmarks, and highlights share one segmented sheet instead of
  being split across separate chrome buttons with highlights unreachable
  entirely, and the contents list now carries a page number per chapter
- Reader: highlights on the page can be tapped, opening a menu anchored to the
  passage — recolour, note, copy, share, remove. The server, the REST API, and
  the Swift service already supported notes, recolouring, and deletion; the
  reader could only ever *create* a highlight, so every one of those was
  unreachable. Notes are written against the quoted passage and mark the
  passage on the page, so an annotated line is findable without remembering it
- iOS app: books can be added to a shelf from inside the shelf — search or
  browse the library, select any number, add them in one call. Filling a shelf
  previously meant leaving it and adding books one at a time from each book's
  own detail screen
- iOS app: journal entries can be edited. The server has supported
  `PATCH /api/journals/{id}` all along, but the app only created and deleted —
  so "Save as draft" was a one-way trip with no way to finish or publish it

### Fixed

- Reader: chapter navigation no longer twitches. `displaySettled` lands a
  first pass, waits for fonts and injected CSS, then re-displays to correct the
  landing — and that correction was visible on every TOC jump. It now happens
  behind a faded stage, the same settled-reveal the book-open path already used.
  Gated on `allowScriptedContent`, since without scripts in the section iframe
  `fonts.ready` never settles and the correction would only land on a 1.5s
  fail-safe — too long to hold a blank stage, so those builds keep the old
  behaviour
- iOS app: tapping the Search tab while already on it now returns to the plain
  search page, unwinding any pushed authors/series/tags screens and clearing
  the query. The tab bar previously swallowed a tap on the current tab, so the
  standard iOS "return to this tab's root" gesture did nothing
- iOS app: saving metadata overrides failed for every book. The client sent
  `creators` as an array of plain strings, but the wire type is
  `Vec<Contributor>` (objects), so `Json<MetadataOverrides>` extraction
  rejected the request before it reached validation — and since the Authors
  field is always prefilled from the book, no edit could ever be saved
- iOS app: the metadata editor submitted every field on every save, writing
  overrides for fields nobody touched and pinning scanned values against
  future rescans. It now sends only what actually changed
- iOS app: metadata fields used their placeholder as their only label, so a
  filled row lost its name — a bare "en" with nothing to say it meant Language

### Changed

- iOS app: gave the native app the Atrium identity it was missing — an
  editorial masthead (drawn mark, italic serif wordmark, accent rule) on the
  library, the serif display face on every navigation title, a named motion
  vocabulary (`Motion.lift/settle/snap/glide/page`) replacing scattered spring
  literals, cover-to-detail zoom navigation, staggered grid entrances, lit
  covers with a gutter shadow, a book-tipping press, and an ambient wash that
  takes the colour of whatever you're currently reading
- iOS app: rebuilt the book detail screen as a jacket rather than a record —
  the accent halo now sits behind the artwork instead of peaking under the
  navigation bar, the jacket dissolves and parallaxes on scroll, the bar title
  appears only once the real one leaves, and rating / status / shelves /
  offline collapsed from four floating controls into one hairline-divided
  plate with a custom status selector. Details read as a colophon (mono
  small-caps keys) and the description is set in the reading face
- iOS app: book detail now leads with About — the blurb set as prose with a
  raised initial and an expand control — followed by tags, then a rating band,
  status, and shelving. The page is paced for reading about a book rather than
  packed for reach, since anyone on this screen has chosen not to be reading
- iOS app: journal entries render their markdown instead of showing the raw
  source, and are drawn as marginal notes against a rule — accent for your own,
  amber for drafts — in the reading face, with long entries collapsed behind a
  More control. The empty state invites writing rather than reporting "No
  entries yet"
- iOS app: "In your library" and "Details" dropped their filled panels for
  hairline records on the page ground, matching the sections above them; the
  short colophon no longer hides behind a disclosure chevron
- iOS app: ratings are set by dragging across the stars, in half-star steps,
  with the value tracking your finger. A tap resolves through the same mapping,
  so the left half of the third star is 2.5 — previously half stars needed a
  second tap on a star you had already chosen
- iOS app: brought the shelves flow onto the shared design language — the shelf
  header leads with the accent rule and a mono meta line, the "add to shelf"
  picker and the shelf composer dropped their stock `List`/`Form` chrome for
  the app's plates, records, and pill selectors, and an empty manual shelf now
  invites filling instead of pointing elsewhere. The plate vocabulary moved out
  of the book-detail feature into `Design/Components/Plate.swift`, since the
  metadata editor and shelves both build on it
- iOS app: rebuilt the metadata editor as an editable colophon matching the
  detail screen's Details plate — persistent mono small-caps labels, per-field
  accent markers showing what you've changed, a Save that stays disabled until
  something differs, and authors as removable chips with an add field instead
  of one comma-delimited string

## [v0.11.4] - 2026-07-24

### Changed

- Added missing row `LIMIT` to `list_wishlist` (#1259)
- Added test coverage for `ScanError` `Lookup`/`Physical`/`Sqlx` variants (#1263)

## [v0.11.3] - 2026-07-24

### Changed

- Added test coverage for `server::backend::covers` (#1262)

## [v0.11.2] - 2026-07-24

### Fixed

- Fixed `deny.toml` `skip-tree` comment for `rustc-hash` (#1276)

## [v0.11.1] - 2026-07-24

### Changed

- Enforced `-D warnings` and wasm32 clippy in `just lint` (#1270)

## [v0.11.0] - 2026-07-23

### Added

- Added the Google Books API key as a first-class Settings field, used as a
  fallback rung in the check-in ISBN lookup ladder (#1292)
