# 02 — Error handling

The shape rule: **predictable failure space → `thiserror`; unpredictable
failure space → `anyhow`.** See
[05-rust-style.md](05-rust-style.md#errors) for the full guidance and
[docs/style-guide.md](../../docs/style-guide.md#errors) for rationale
and worked examples.

## When to use `thiserror`

When you can enumerate the ways the function will fail and a caller
might branch on them. Auth, validation, parsing, API boundary checks —
anywhere a UI renders a per-case message. Define a typed error enum in
the module that owns the failure. Use `#[error(transparent)]` + `#[from]`
to wrap lower-level errors (`sqlx`, `std::io`, etc.) so `?` propagates
cleanly.

**Coarse variants.** Group by failure mode, let the `#[error("...")]`
message carry the detail. Don't split `PasswordTooShort` from
`PasswordTooLong` unless a caller actually branches on them — one
`Validation(String)` variant is usually enough.

```rust
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("file not found: {path}")]
    NotFound { path: PathBuf },
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

## When to use `anyhow`

When the failure source is a foreign system that can fail in arbitrary
ways (filesystem walks, EPUB parsing, network fetches) and the caller
just propagates. `anyhow::bail!` with a context-rich format string is
honest about the open-ended failure space:

```rust
pub async fn reindex(pool: &SqlitePool, library_path: &str) -> anyhow::Result<ReindexStats> {
    let stat = scan(library_path).await?;
    if let Some(msg) = stat.error {
        anyhow::bail!("scan of {library_path} failed: {msg}");
    }
    Ok(stats)
}
```

Application-level propagation in handlers also uses `anyhow` — the
handler signature returns `anyhow::Error` and the body upgrades typed
errors via `?`.

```rust
async fn trigger_scan(State(s): State<AppState>) -> Result<StatusCode, anyhow::Error> {
    scanner::scan(&s.pool).await?;
    Ok(StatusCode::OK)
}
```

## Boundary rule

**Never return raw `sqlx::Error` across a module boundary.** Wrap it
via `#[error(transparent)] #[from]` on the module's error enum, or
convert into `anyhow::Error` with `.with_context(...)`. Leaking
`sqlx::Error` propagates an implementation detail (the DB crate) into
every downstream caller.

## `unwrap` / `expect`

Banned in production paths — only in tests or truly-infallible setup
code.
