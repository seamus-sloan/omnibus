# 02 — Error handling

- **Domain / library errors:** use `thiserror` to define typed error enums per module (e.g. `DbError`, `ScanError`). Each variant gets a meaningful `#[error("...")]` message.
- **Application-level propagation:** use `anyhow` in handlers and top-level code where the specific error type doesn't need to be matched.
- Use `#[error(transparent)]` + `#[from]` for wrapping lower-level errors (`sqlx`, `std::io`, etc.) so `?` propagates cleanly.

```rust
// Domain error — in a library module
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("file not found: {path}")]
    NotFound { path: PathBuf },
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// Handler — anyhow for propagation, no need to enumerate variants
async fn trigger_scan(State(s): State<AppState>) -> Result<StatusCode, anyhow::Error> {
    scanner::scan(&s.pool).await?;
    Ok(StatusCode::OK)
}
```

Do **not** use `unwrap()` / `expect()` in production paths — only in tests or truly-infallible setup code.
