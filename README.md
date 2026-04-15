# omnibus

Minimal full-stack Rust app using Dioxus, Axum, and SQLite.

## Development environment (Nix)

Use Nix to provide all system dependencies:

```bash
nix develop
```

If you do not use flakes:

```bash
nix-shell
```

Then run all commands from inside the shell.

## Run the app

```bash
cargo run
```

Server starts on `http://127.0.0.1:3000` by default.

Environment variables:
- `PORT` (default: `3000`)
- `DATABASE_URL` (default: `sqlite://omnibus.db?mode=rwc`)

## Test

```bash
cargo test
```

### Optional rough Playwright E2E tests (Rust)

1. Start the app (`cargo run`)
2. Install Playwright browsers for Rust Playwright setup (outside of Cargo)
3. Run:

```bash
cargo test --features e2e -- --ignored
```
