# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

**How this file is maintained:** [`.github/workflows/release.yml`](.github/workflows/release.yml)
cuts an automated GitHub release (and version tag) on every merge to `main` —
a patch bump by default, a minor bump with the `minor version` label, and no
release for a PR labeled `no release`. An unlabeled PR that only touches
docs/CI files also skips the release — but an explicit `patch version` or
`minor version` label always wins and still cuts one, even for a docs/CI-only
PR. That gives
every release a tag and generated release notes, but not every one of those
automated releases is worth a line here. This file is a curated, human-written
summary of user-facing changes, updated as part of notable PRs rather than
generated from the release automation. It was started retroactively and does
not attempt to reconstruct the project's full release history — only the most
recent releases are recorded below; everything earlier is available via the
[GitHub releases page](https://github.com/seamus-sloan/omnibus/releases) and
`git log`.

## [Unreleased]

### Added

- **Fetch metadata** on the metadata edit page: one button opens a search
  that has already asked every configured provider for this book, and lists
  what each returned — cover, title, authors, year, publisher, and which
  source it came from — with the editions that actually match what you asked
  for at the top, and in an order the providers don't decide, so one being
  slow or unreachable never reshuffles the list. A footer says what each
  source contributed, so "unreachable" never reads as "no results" (#1661)
- Picking an edition shows **only the fields it would change** — usually two
  or three rather than a full record — each with an arrow that copies it into
  the form, plus take-all and a "show all fields" toggle. Copies are staged,
  so the form's own Save is still what writes them, and a field the source has
  no value for cannot be applied at all: a provider that doesn't know a field
  can never blank out one you already have (#1662)
- **Book #** is now one of the fields the compare view can copy. Hardcover is
  the only source that publishes a book's position in its series, so a book
  numbered there can be numbered here in one click (#1665)

### Changed

- **The metadata search takes a title, an author, and an ISBN separately.** One
  box could not say which part was which, so the search had to guess — and
  guessing wrong is what sent "Dune Frank Herbert" to Open Library as a title
  and got back five books written *about* Dune. Three fields, each seeded from
  the book, and an ISBN alone is now a valid search
- **Fetch metadata now asks each source in its own terms.** The search used to
  flatten the book's title and author into one phrase and hand that to every
  provider, which meant Open Library searched the author's name *inside the
  title field* — asking for "Dune Frank Herbert" returned five books written
  **about** Dune and not the novel — and Hardcover, whose title filter is
  exact-match only, matched nothing at all for any book that has an author.
  Title, author, and the ISBN already sitting in the edit form now travel
  separately, Hardcover goes through its own full-text search endpoint, and an
  ISBN routes every source straight to the exact edition
- **Results are filtered, not just ordered.** Study guides, summaries, and
  "analysis of" editions are dropped rather than ranked below the book, a
  mistyped title still finds its match, and coincidental matches no longer
  fill the list. Each candidate is scored from the candidate and your query
  alone — never from which source answered — so a source being slow or
  unreachable removes its rows and reorders nothing else
- **Editions without an ISBN now show up.** A source that describes a *work*
  rather than one printing — which is most of what Hardcover's search returns,
  and how Open Library files older or uncatalogued books — used to have its
  candidates discarded silently, with nothing on screen to say so. They are
  listed now, and selecting one still fetches its full record
- Searching by **ISBN alone**, or by **author alone**, now works — both were
  rejected or silently mangled before
- The picker **no longer searches the moment it opens**. All three fields are
  filled in from the book, including its ISBN, and you press Search. An ISBN
  narrows every source to that one edition, so seeing it sitting there — and
  being able to clear it first — is the difference between a short list you
  asked for and one you can't explain
- **A source that rate-limits us is left alone until it recovers.** Its row
  says "rate limited, skipping for 10m" rather than "unavailable", and the search
  no longer spends a request re-asking a source that has already refused —
  which on Google Books' free tier could otherwise keep failing for hours
- The metadata edit page now has one fetch-from-outside action instead of
  three: "Fetch metadata" replaces both the "Fetch from Hardcover" panel
  (one provider, one field at a time) and that page's "Fetch Summary" button
  (one field), since it covers what each of them did and more. Fetch Summary
  is unchanged on the book detail page
- The edit form no longer breaks its own labels mid-word ("ISBN-/13") on a
  narrow window, keeps Save and Discard reachable above the phone tab bar
  instead of behind it, and collapses to a single column on a phone
- The compare view's cover row applies the source's cover art. Unlike every
  other row it writes immediately — the browser can't fetch a provider's image
  cross-origin, so the server does — and says so; the new cover appears on the
  edit page, the detail page, and the grid without a reload, and "revert to
  scanned cover" still works afterwards. The server only fetches from hosts
  the provider catalog publishes, over HTTPS, re-checking every redirect hop,
  with a size cap and an image-format check before anything is written
  (#1663)

### Fixed

- Covers now render on a Kobo for books whose cover was replaced by hand. A
  user-uploaded WebP cover was stored as-is and served verbatim to the device,
  which renders no cover for a WebP; override covers are now converted to JPEG
  on upload, and downscaled if oversized. PNG and GIF uploads are unchanged
  (#2116)
- Audiobook-only books are no longer offered to a Kobo. A book with no EPUB
  and no CBZ on a Kobo-synced shelf used to sync as an entitlement the device
  could never download, so it retried the failing download indefinitely; such
  books are now excluded from the sync set, and one a device already holds is
  archived on its next sync (#2116)
- Provider cover images no longer fail to load behind the content-security
  policy: the `img-src` allowlist is now derived from the provider catalog and
  includes the redirect hops Open Library's cover CDN and Hardcover's asset
  host actually serve bytes from (#1661)
- Rotating the device and back in the iOS EPUB reader no longer drifts off
  the page the reader was actually on: a rotation's re-pagination now goes
  through the same correction (and echo tagging) a boot restore does, so it
  writes no position and never moves the restore anchor. Added an explicit
  single/two-page reader setting on iOS, matching web (#2081)
- Uploading a book from the iOS app works. Every upload previously failed with
  "title and author are required to file the book", because the app posted the
  file straight to the commit endpoint and skipped the confirm step those
  fields come from. It now reads the file's embedded metadata, shows it for you
  to correct, and files the book under what you confirmed. The file picker is
  narrowed to the formats the server accepts, and anything else it still lets
  through is refused before a byte is sent rather than after. Picking several
  MP3s from one folder adds a single audiobook with all its parts, instead of
  one book per file (#2100)
- Raising the keyboard on iOS no longer drags the tab bar up with it: the
  keyboard now covers the bar instead of stacking on top of it, while fields
  inside a tab keep their keyboard avoidance (#2102)

## [0.22.10] - 2026-08-18

### Fixed

- Bumped the `h2` dependency to 0.4.16 to address RUSTSEC-2026-0258 (#2026)

## [0.22.9] - 2026-08-18

### Fixed

- Batched cross-format `audio_marks` queries and deduplicated `alignment_view`
  calls (#2019, #2023)

[Unreleased]: https://github.com/seamus-sloan/omnibus/compare/v0.22.10...HEAD
[0.22.10]: https://github.com/seamus-sloan/omnibus/compare/v0.22.9...v0.22.10
[0.22.9]: https://github.com/seamus-sloan/omnibus/releases/tag/v0.22.9
