# 01 — Dev environment

All system dependencies (Rust toolchain, SQLite, pkg-config, OpenSSL, Node.js, Android SDK/NDK, JDK) are provided by Nix. **Always** work inside the dev shell:

```bash
nix develop --command zsh   # preferred — keeps your shell prompt intact
nix develop                 # also works; spawns a bash subshell
```

The shell hook sets:

- `DATABASE_URL=sqlite://omnibus.db?mode=rwc`
- `PLAYWRIGHT_BROWSERS_PATH` → Nix-provided Chromium (don't run `npx playwright install`)
- `ANDROID_HOME` and `ANDROID_NDK_HOME` (auto-detected from standard Android Studio install paths)

Override `PORT` (default `3000`) if you need a different port.

If `ANDROID_HOME` / `ANDROID_NDK_HOME` come back empty, set them manually:

```bash
export ANDROID_HOME=$HOME/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
```

If a task requires a new system dependency, add it to [flake.nix](../../flake.nix) rather than documenting a manual install step.
