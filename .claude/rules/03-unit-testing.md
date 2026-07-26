# 03 — Unit & integration testing

Testing is a first-class requirement. Every meaningful behavior should
have a test at the **lowest applicable level**. See
[05-rust-style.md](05-rust-style.md#tests) for the broader style
guidance and [docs/style-guide.md](../../docs/style-guide.md#tests)
for rationale.

## Where tests live

- **Unit tests (non-trivial):** sibling file `<mod>/tests.rs`. Promote
  a module's tests to a sibling file whenever they cross "trivial" —
  more than one or two short cases, or any shared helper that would
  otherwise pad the prod file.
  [db/src/books/tests.rs](../../db/src/books/tests.rs) is the model.
- **Unit tests (trivial):** inline `#[cfg(test)] mod tests` at the
  bottom of the file is fine for 1–2 cases that need no helpers.
- **Integration tests:** sibling `<module>/tests.rs` next to the
  handler module (e.g.
  [server/src/backend/progress/tests.rs](../../server/src/backend/progress/tests.rs));
  inline `#[cfg(test)]` only for routes still in `server/src/backend.rs`
  itself. Drive `rest_router(AppState::new(pool))` via
  `tower::ServiceExt::oneshot` against an in-memory DB.
- **E2E tests:** see [04-playwright.md](04-playwright.md).

All tests use `sqlite::memory:` for isolation — never the on-disk DB.

## Shared helpers

Cross-cutting helpers — in-memory pool init, seed factories, fixture
builders — live in a `test_support` module, never duplicated across
files (no per-file `make_test_dir()` / `seed_minimal_books()`). Put it
at the scope its helpers serve:

- **Crate-wide** → `<crate>/src/test_support.rs`, gated
  `#[cfg(any(test, feature = "test-support"))]`, as in
  [db/src/test_support.rs](../../db/src/test_support.rs). Only this
  shape is reusable from another crate.
- **One module tree** → a sibling `test_support.rs` next to the code
  it serves, e.g. `server/src/backend/test_support.rs` (REST fixtures)
  and `server/src/auth/test_support.rs` (user + token factories).

To reuse a crate-wide one elsewhere, depend on it with the feature on —
`omnibus-db = { path = "../db", features = ["test-support"] }` under
`[dev-dependencies]`, which cargo unifies into test builds only.

## No ambient environment

A test must never depend on an env var it didn't set. Every var with an
`effective_*` / seeds-if-unset fallback (`HARDCOVER_API_KEY`, `SMTP_*`,
`EBOOK_LIBRARY_PATH`, …) is set in a developer's gitignored `.env` and
absent in CI, so a test that assumes "unset" passes in CI and fails
locally. Pin it with `EnvVarGuard` from `omnibus_db::test_support` —
`EnvVarGuard::set("HARDCOVER_API_KEY", None)` removes the var for the
test and restores it on drop, under a process-wide lock. Never
hand-roll a set/restore guard.

## Coverage expectations

- **Every `pub` fn:** one happy-path test, plus one test per
  `thiserror` variant the function can return. For
  `anyhow`-returning functions, cover happy + one representative
  failure.
- **Edge / boundary tests** only for functions doing tricky math,
  parsing, or arithmetic. Don't multiply for `Some` vs `None`,
  empty-string vs whitespace, etc. unless the function actually
  branches on them.
- **`server::backend` handlers:** 200 success, 4xx client errors,
  5xx DB-failure paths.
- **`frontend::pages` components with logic:** rendered output
  contains expected content.
- **`frontend::rpc` server functions:** thin wrappers — covered
  transitively by db tests; only add a direct test if the wrapper
  composes multiple db calls non-trivially.
- **New features** must not ship without tests covering their
  acceptance criteria from the relevant roadmap initiative.

## Naming

Long sentence style, `fn_under_test_does_X_when_Y`. The test name is
the spec; "FAILED `search_books_finds_by_title_and_ranks_by_bm25`"
tells you what's broken without opening the file.

## Shape

```rust
// db/src/books/tests.rs
use super::*;
use crate::test_support::{new_in_memory_pool, seed_minimal_books};

#[tokio::test]
async fn get_book_returns_record_for_known_id() { ... }

#[tokio::test]
async fn get_book_returns_none_for_missing_id() { ... }

#[tokio::test]
async fn get_book_propagates_db_error_when_pool_is_closed() { ... }
```

## Running

```bash
just test                                          # full matrix (db + server + frontend(server) + shared)
cargo test -p omnibus                              # /api/* REST integration tests
cargo test -p omnibus-db                           # db + scanner + sync tests
cargo test -p omnibus-frontend --features server   # rpc + page tests
cargo test -p omnibus-shared                       # shared serde / ebook / progress tests
cargo test -p <crate> <test_name>                  # single test by name
```
