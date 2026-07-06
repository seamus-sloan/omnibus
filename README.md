<div align="center">

<img src="frontend/assets/omnibus-stoat.png" alt="Omnibus" width="120" />

# Omnibus

**The Plex / Jellyfin for your books.**
A self-hosted ebook & audiobook library — read in the browser, listen anywhere, and browse a collection that belongs entirely to you.

Built with Rust ([Axum](https://github.com/tokio-rs/axum) + [Dioxus](https://dioxuslabs.com/)), SQLite, and a native iOS / Android shell.

[![Clippy & Tests](https://img.shields.io/github/actions/workflow/status/seamus-sloan/omnibus/rust.yml?branch=main&label=Clippy%20%26%20Tests&logo=rust&logoColor=white)](https://github.com/seamus-sloan/omnibus/actions/workflows/rust.yml)
[![Playwright](https://img.shields.io/github/actions/workflow/status/seamus-sloan/omnibus/e2e.yml?branch=main&label=Playwright&logo=playwright&logoColor=white)](https://github.com/seamus-sloan/omnibus/actions/workflows/e2e.yml)
[![CSS Lint](https://img.shields.io/github/actions/workflow/status/seamus-sloan/omnibus/css-lint.yml?branch=main&label=CSS%20Lint)](https://github.com/seamus-sloan/omnibus/actions/workflows/css-lint.yml)
[![Docker Hub](https://img.shields.io/docker/v/sesloan/omnibus?sort=semver&logo=docker&logoColor=white&label=docker%20hub&color=2496ED)](https://hub.docker.com/r/sesloan/omnibus/tags)
[![Image size](https://img.shields.io/docker/image-size/sesloan/omnibus?sort=semver&logo=docker&logoColor=white&label=image&color=2496ED)](https://hub.docker.com/r/sesloan/omnibus/tags)
[![Docker pulls](https://img.shields.io/docker/pulls/sesloan/omnibus?logo=docker&logoColor=white&color=2496ED)](https://hub.docker.com/r/sesloan/omnibus)

</div>

<div align="center">
<img src="docs/screenshots/library.png" alt="Omnibus library — cover grid" width="90%" />
</div>

> [!NOTE]
> **Status: early development.** Foundations and browse/discovery have shipped;
> reading/listening is in progress. The landing grid/table, EPUB reader,
> audiobook player, command-palette search, auth, and author/series/tag
> discovery are live. See the [roadmap](docs/roadmap/0-0-summary.md) for what's
> next.

## Features

- 📚 **A real library, not a file list** — cover-art grid or dense metadata table, plus smart & shared shelves that update themselves.
- 📖 **In-browser EPUB reader** — pick up where you left off, progress synced server-side.
- 🖍️ **Highlights & quote cards** — save passages and export a shareable quote card in a tap.
- 📤 **Send to your device** — push any ebook straight to your Kobo or Kindle, no Calibre round-trip.
- 🎧 **Audiobook player** — HLS-transcoded streaming with a chapter map, resumable across devices.
- ⌘ **Command-palette search** — `⌘K` for full-text search across titles, authors, series, and tags.
- 🧭 **Rich book pages** — your own ratings, reading journals, series tracking, and reading insights on your own database.
- 🔗 **Discovery** — browse by author, series, and tag; "readers also enjoyed" suggestions via Hardcover.
- 📱 **Native mobile app** — an iOS / Android shell over the same backend.
- 🐳 **Self-hosted, Jellyfin-style** — bind-mount your library, one `docker compose up`.

## Screenshots

<div align="center">

| Read & highlight | Listen |
|:---:|:---:|
| <img src="docs/screenshots/reader.png" alt="EPUB reader with quote-card composer" height="250" /> | <img src="docs/screenshots/player.png" alt="Audiobook player with chapter map" height="250" /> |
| **Search everything** | **Every book, in depth** |
| <img src="docs/screenshots/search.png" alt="Command-palette search" height="250" /> | <img src="docs/screenshots/discovery.png" alt="Book detail page" height="250" /> |

</div>

## Deploy with Docker

The fastest way to run Omnibus. The image is published to
[Docker Hub as `sesloan/omnibus`](https://hub.docker.com/r/sesloan/omnibus/tags),
and the repo ships a Jellyfin-style [`docker-compose.yml`](docker-compose.yml):
bind-mount your library read-only, durable state in `/config`, regenerable cache
in `/cache`.

```bash
# 1. Point the library mounts at your books and set your access URL.
$EDITOR docker-compose.yml

# 2. Build the bundle and start it (first build compiles the workspace + WASM).
docker compose up -d --build

# 3. Open http://localhost:3000 and register — the first account is the admin.
```

Full deployment reference — volumes, env vars, reverse-proxy/TLS, PUID/PGID, and
admin recovery — is in [docs/docker.md](docs/docker.md).

## Develop

Development uses [Nix](https://wiki.nixos.org/wiki/NixOS_Wiki) to pin every
dependency (Rust toolchain, SQLite, Node, mobile SDKs). **Always work inside the
dev shell.**

```bash
nix develop                 # slim default shell (spawns bash)
nix develop --command zsh   # …keeping your shell
direnv allow                # or auto-load via direnv (once)
```

**The `just` recipes are the quickest way in.** They self-wrap in the right Nix
shell, so they run straight from the slim `default` shell:

```bash
just serve       # full stack in Zellij — server + web + mobile + Playwright panes
just serve-pc    # …the same stack via process-compose
just dev-up      # just the web server: port-walks, seeds the admin user
just dev-bounce  # cleanly restart a wedged dev server (e.g. after a migration)
```

`just serve` / `just serve-pc` multiplex every platform into one session using
the shared [`justfile`](justfile). Prefer to drive the pieces yourself?

```bash
dx serve --platform web -p omnibus   # fullstack SSR + WASM at http://localhost:8080
cargo run -p omnibus                 # server only (native backend) at http://0.0.0.0:3000
```

### Nix shells

The flake exposes purpose-built shells so daily cargo work doesn't pull in
Playwright + mobile + audit tooling. Opt into a heavier shell only when you need
it (the `just` recipes above pick the right one for you):

| Shell | When to use |
|---|---|
| `default` | Daily `cargo` / `clippy` / `test` / editor — what direnv auto-loads |
| `.#web` | `dx serve --platform web`, `just dev-up`, anything that bundles WASM |
| `.#e2e` | `npx playwright test` (the Chromium bundle lives here) |
| `.#mobile` | Android / iOS builds (Rust cross-targets + JDK + Android NDK detect) |
| `.#audit` | `cargo audit` / `cargo deny` |

| Variable | Default |
|---|---|
| `PORT` | `3000` |
| `DATABASE_URL` | `sqlite://omnibus.db?mode=rwc` |

Secret-bearing config lives in a gitignored `.env` — copy [`.env.example`](.env.example)
on first checkout. It's the canonical, annotated reference for every supported
variable.

## Mobile app

The mobile app is a Dioxus Native shell that connects to `http://127.0.0.1:3000`,
so **start the server first** (`just serve` handles this). The iOS and Android
panes inside `just serve` build and launch the app for you; the one-time platform
setup below still applies.

### iOS Simulator

Requires macOS with Xcode and at least one iOS Simulator installed (add one via
**Xcode → Window → Devices and Simulators**). The iOS pane then launches the
simulator and installs the app with no extra commands.

### Android Emulator

1. **Install Android Studio** — [developer.android.com/studio](https://developer.android.com/studio)
2. **Install the NDK** — **Tools → SDK Manager → SDK Tools → check "NDK (Side by side)" → Apply**
3. **Create an emulator** — **Tools → Device Manager → Create Virtual Device** (API 33+ recommended), then start it.
4. **Enter the mobile dev shell** — it auto-detects `ANDROID_HOME` / `ANDROID_NDK_HOME`:

   ```bash
   nix develop .#mobile --command zsh
   echo $ANDROID_HOME       # e.g. /Users/<you>/Library/Android/sdk
   echo $ANDROID_NDK_HOME   # e.g. …/sdk/ndk/28.x.x
   ```

   If they're empty, set them manually:

   ```bash
   export ANDROID_HOME=$HOME/Library/Android/sdk
   export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
   ```

5. If the app can't reach the server after boot, run `adb reverse tcp:3000 tcp:3000`.

## Tests

```bash
just test    # full matrix: db + server + frontend(--features server) + shared
just lint    # cargo fmt --check + clippy across the crate/feature matrix + stylelint
just check   # lint then test

# Web E2E (Playwright — server must be running; Chromium comes from Nix)
cd ui_tests/playwright
npm install                 # first time only; do NOT run `npx playwright install`
npx playwright test
```

*Mobile UI tests will land later using [MobileWright](https://mobilewright.dev/).*

## Project layout

Five-crate Cargo workspace:

| Crate | Responsibility |
|---|---|
| `shared/` | serde types shared across crates |
| `db/` | data layer, indexer, migrations |
| `frontend/` | Dioxus UI + server functions |
| `server/` | fullstack binary + REST router |
| `mobile/` | thin native shell |

Deeper architecture — module maps, request-flow diagrams, mobile-auth — is in
[.claude/architecture.md](.claude/architecture.md). Contributor conventions live
under [.claude/](.claude/) (dev environment, error handling, testing, migrations,
style).

## Roadmap

See [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md) for the phased plan
— foundations, browse/discovery, reading/listening, personalization, device sync,
admin, and mobile.
