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

### Fixed

- Rotating the device and back in the iOS EPUB reader no longer drifts off
  the page the reader was actually on: a rotation's re-pagination now goes
  through the same correction (and echo tagging) a boot restore does, so it
  writes no position and never moves the restore anchor. Added an explicit
  single/two-page reader setting on iOS, matching web (#2081)

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
