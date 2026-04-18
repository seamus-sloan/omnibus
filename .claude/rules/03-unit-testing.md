# 03 — Unit & integration testing

Testing is a first-class requirement. Every meaningful behavior should have a test at the **lowest applicable level**.

## Where tests live

- **Unit tests:** inline `#[cfg(test)]` modules in the same file as the code under test. This is the default for all logic in `db/`, `scanner/`, and `frontend/`.
- **Integration tests:** inline `#[cfg(test)]` in `backend.rs`, using `tower::ServiceExt::oneshot` to drive full request/response cycles against an in-memory DB.
- **E2E tests:** see [04-playwright.md](04-playwright.md).

All tests use `sqlite::memory:` for isolation — never the on-disk DB.

## Coverage expectations

- **`db/` functions:** happy path + not-found / missing + constraint violations.
- **`backend/` handlers:** 200 success, 4xx client errors, 5xx DB-failure paths.
- **`frontend/` components with logic:** rendered output contains expected content.
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
cargo test -p omnibus                    # all server tests
cargo test -p omnibus <test_name>        # single test by name
```
