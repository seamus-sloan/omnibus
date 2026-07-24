# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Releases are cut automatically on merge to `main` (see
`.github/pull_request_template.md` — unlabeled PRs default to a patch
release; `minor version` / `patch version` / `no release` labels
override that), so entries here are curated manually rather than
generated from the raw commit log.

## [Unreleased]

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
