# 05 — Rust style

The normative rules. For rationale, before/after examples, and the *why*
behind each rule, see [docs/style-guide.md](../../docs/style-guide.md).

This rule applies to all Rust code in the workspace (`db/`, `server/`,
`frontend/`, `shared/`, `mobile/`). TypeScript / Playwright style is
covered by [04-playwright.md](04-playwright.md).

## Comments & docs

- **Module `//!`** is required on every file. Keep it to one short
  paragraph (≤ 5 lines): what this module is for, who calls it. No
  references to roadmap phases or issue numbers — those belong in commit
  messages and PR bodies, where they don't rot.
- **Function `///`** is required on every `pub` item: one-line summary.
  Add a longer rustdoc block (params, errors, invariants) only when the
  behaviour is genuinely surprising — locks, retries, side effects, or
  contracts a caller can break by accident.
- **Inline `//` inside function bodies** is for explaining **why**, never
  **what**. A non-obvious invariant, a workaround for a specific bug, a
  perf trick. If you're narrating the algorithm, the function probably
  needs to be split (see below). Section-marker comments
  (`// --- Removed`) are tolerated *only* when a function can't be split
  further and still needs nav aids.
- **Keep comments terse — one line wins.** State the *why* and stop;
  don't restate what the code or attribute names already say, and don't
  leave PR-narration (the play-by-play of what a change does, worked
  examples, before/after) in the source — that belongs in the PR body,
  not a multi-line block above a one-line tweak.

## Function shape

- **Soft cap: ~80 lines per function.** Once a function crosses it,
  extract named helpers. Same applies to functions with clear staged
  sub-steps or stages that are independently testable.
- Model: [db/src/worker/](../../db/src/worker/) (small `impl`
  methods). Resolved anti-pattern: `sync_books`'s old ~270-line body
  was extracted into per-bucket helpers under
  [db/src/sync/](../../db/src/sync/) (`sync.rs` is now a 46-line module
  file) — the shape this rule prescribes.

## File shape

- **Soft cap: ~800 lines per file.** When a file crosses it, split by
  sub-topic using the `books/` subdirectory pattern — e.g.
  `sync.rs` → `sync/{mod,books,authors,backfill}.rs`.
- Module-name choice mirrors the responsibilities, not the line count.

## Errors

- **Predictable failure space → `thiserror` enum** in the same module.
  Anything where callers branch on the failure, or a UI renders a
  per-case message: auth, validation, parsing, API boundary checks.
- **Unpredictable failure space → `anyhow`** with a contextual message
  (`anyhow::bail!("scan of {path} failed: {msg}")`). The right fit when
  the source is a foreign system (filesystem, parser, network) and the
  caller just propagates. Example: `reindex` in
  [db/src/indexer.rs](../../db/src/indexer.rs).
- **Coarse variants.** Group by failure mode and let the
  `#[error("...")]` message carry the detail. One
  `PasswordInvalid(String)` beats `PasswordTooShort` /
  `PasswordTooLong` / `PasswordInCommonList` unless the caller actually
  branches on them.
- **Never** return raw `sqlx::Error` across a module boundary. Wrap it
  with `#[error(transparent)] #[from]` on a module-local enum, or
  convert into `anyhow::Error` with `.with_context(...)`.
- **Handlers** stay on `anyhow::Error` at the signature; the body
  upgrades typed errors via `?`.

See [02-error-handling.md](02-error-handling.md) for the underlying rule.

## Tests

- **Placement**: sibling file `<mod>/tests.rs` for anything non-trivial
  ([db/src/books/tests.rs](../../db/src/books/tests.rs) pattern). Inline
  `#[cfg(test)] mod tests` is allowed only for tiny modules with 1–2
  trivial tests.
- **Shared helpers**: every crate has a `test_support` module
  (`<crate>/src/test_support.rs`, gated
  `#[cfg(any(test, feature = "test-support"))]`) for in-memory pool
  init, seed factories, common fixtures. No duplicated
  `make_test_dir()` / `seed_minimal_books()` across files.
- **Coverage rule**: every `pub` fn gets *one happy-path test plus one
  test per `thiserror` variant it can return*. Skip edge / boundary
  tests unless the function does tricky math, parsing, or arithmetic.
  For `anyhow`-returning functions, cover happy + one representative
  failure.
- **Naming**: long sentence style,
  `fn_under_test_does_X_when_Y`. E.g.
  `search_books_finds_by_title_and_ranks_by_bm25`, not
  `finds_by_title`.

See [03-unit-testing.md](03-unit-testing.md) for the underlying rule.

## Visibility & types

- **Visibility**: default `pub` is fine. The module structure carries the
  surface — don't audit-swap to `pub(crate)`, and don't write narrower
  scopes (`pub(super)`) prophylactically. Use `pub(crate)` only when you
  have a specific reason a type *must* not leak.
- **Shared types**: anything used by 2+ crates lives in `shared/`. Hoist
  when the second consumer appears, not before.
- **DB row types vs wire DTOs**: when the same record needs to cross the
  HTTP/RPC boundary, it's fine to have a `db::Foo` and a `shared::Foo`
  that look similar, with `From`/`Into` at the boundary. Don't try to
  make one type serve both.

## Mechanics

- **Imports**: blocks separated by blank lines — `std`, then external
  crates, then `crate::`, then (when present) `super::` — each block
  alphabetical. External crates merge into one block regardless of
  `#[cfg]` gating: a gated and an ungated `use` from the same crate (or
  two different crates) still belong in the same block, attribute kept
  attached to its own `use` line. `crate::` and `super::` are a split
  4th block by house convention (an audit under #1455 found 30+ files
  independently doing this) rather than merged into one "local" block —
  keep that split when adding imports to an existing file, and use it
  for new files that import from both. Convention-only (we don't depend
  on nightly rustfmt). Don't add a `rustfmt.toml` group setting that
  requires nightly.
- **`unwrap()` / `expect()`** are banned in production paths (see
  [02-error-handling.md](02-error-handling.md)). Test code can use
  freely.
- **`unsafe`**: avoid it in production code. Any `unsafe` block must
  document the invariant in a `// SAFETY:` comment, and new production
  uses must be raised in the PR description.

## Out of scope

- TypeScript / Playwright — see [04-playwright.md](04-playwright.md).
- SQL migrations — see [06-migrations.md](06-migrations.md).
