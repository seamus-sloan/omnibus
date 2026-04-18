# 03 — Unit & integration testing

Testing is a first-class requirement. Every meaningful behavior should have a test at the **lowest applicable level**.

## Where tests live

- **Unit tests:** inline `#[cfg(test)]` modules in the same file as the code under test. This is the default for all logic in `frontend/src/db.rs`, `frontend/src/scanner.rs`, and the `frontend/src/pages/` components.
- **Integration tests:** inline `#[cfg(test)]` in `server/src/backend.rs`, driving `rest_router(AppState::new(pool))` via `tower::ServiceExt::oneshot` against an in-memory DB.
- **E2E tests:** see [04-playwright.md](04-playwright.md).

All tests use `sqlite::memory:` for isolation — never the on-disk DB.

## Coverage expectations

- **`frontend::db` functions:** happy path + not-found / missing + constraint violations.
- **`server::backend` handlers:** 200 success, 4xx client errors, 5xx DB-failure paths.
- **`frontend::pages` components with logic:** rendered output contains expected content.
- **`frontend::rpc` server functions:** thin wrappers — covered transitively by db tests; only add a direct test if the wrapper composes multiple db calls non-trivially.
- **New features** must not ship without tests covering their acceptance criteria from [ROADMAP.md](../../ROADMAP.md).

## Shape

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_book_returns_correct_record() { ... }

    #[tokio::test]
    async fn get_book_returns_not_found_for_missing_id() { ... }

    #[tokio::test]
    async fn get_book_returns_error_on_db_failure() { ... }
}
```

## Running

```bash
cargo test -p omnibus                              # /api/* REST integration tests
cargo test -p omnibus-frontend --features server   # db + scanner + rpc + page tests
cargo test -p <crate> <test_name>                  # single test by name
```
