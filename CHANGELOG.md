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

- Edition search on the metadata edit page: one search asks every configured
  provider at once and lists the editions each returned — cover, title,
  authors, year, publisher, and which source it came from — with a per-source
  status line so a provider being unreachable never reads as "no results".
  Selecting an edition opens it in a compare view, re-fetched in full from the
  source it came from (#1661)
- Side-by-side compare on the metadata edit page: your value and the source's,
  one row per field, with an arrow on each row that copies that field into the
  form and a "take everything from this source" shortcut. Copies are staged —
  the form's own Save is still what writes them — and a field the source has
  no value for cannot be applied, so a provider that doesn't know a field can
  never blank out one you already have (#1662)

### Fixed

- Provider cover images no longer fail to load behind the content-security
  policy: the `img-src` allowlist is now derived from the provider catalog and
  includes the redirect hops Open Library's cover CDN and Hardcover's asset
  host actually serve bytes from (#1661)

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
