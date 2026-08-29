<div align="center">

<img src="frontend/assets/omnibus-stoat.png" alt="Omnibus" width="120" />

# Omnibus

**The Plex / Jellyfin for your books.**
A self-hosted ebook & audiobook library — read in the browser, listen anywhere,
and browse a collection that belongs entirely to you.

[![Clippy & Tests](https://img.shields.io/github/actions/workflow/status/seamus-sloan/omnibus/rust.yml?branch=main&label=Clippy%20%26%20Tests&logo=rust&logoColor=white)](https://github.com/seamus-sloan/omnibus/actions/workflows/rust.yml)
[![Playwright](https://img.shields.io/github/actions/workflow/status/seamus-sloan/omnibus/e2e.yml?branch=main&label=Playwright&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCA0MDAgNDAwIj4KPHBhdGggZmlsbD0iIzJFQUQzMyIgZD0iTTM0MS44IDEyOS4yYy0xMi40IDIuMi00Mi4zIDQuOS03OS4yLTUtMzYuOS05LjktNjEuNC0yNy4yLTcxLjEtMzUuMy0xMy44LTExLjUtMTkuOC0xOS41LTI1LjctNy40LTUuMyAxMC43LTEyIDI4LjEtMTguNSA1Mi40LTE0LjEgNTIuNy0yNC43IDE2My44IDYyLjYgMTg3LjIgODcuMiAyMy40IDEzMy43LTc4LjIgMTQ3LjgtMTMwLjkgNi41LTI0LjMgOS40LTQyLjcgMTAuMi01NC42LjktMTMuNC04LjQtOS41LTI2LjEtNi40eiIvPgo8cGF0aCBmaWxsPSIjMUQ4RDIyIiBkPSJNMjI1LjMgMjY5LjJjLTQxLTEyLTQ5LjItNDUuMi00OS4yLTQ1LjJsNTYuOCAxNS45IDMwLjEtMTE1LjZjLTM2LjktOS45LTYxLjctMjcuMy03MS40LTM1LjQtMTMuOC0xMS41LTE5LjgtMTkuNS0yNS43LTcuNC01LjMgMTAuNy0xMiAyOC4xLTE4LjUgNTIuNC0xNC4xIDUyLjctMjQuNyAxNjMuOCA2Mi42IDE4Ny4ybDEuOC40eiIvPgo8cGF0aCBmaWxsPSIjMkQ0NTUyIiBkPSJNMTkzLjkgMTY3LjZjMTEuOSAzLjQgMTguMiAxMS43IDIxLjUgMTkuMWwxMy4yIDMuOHMtMS44LTI1LjgtMjUuMS0zMi40Yy0yMS44LTYuMi0zNS4zIDEyLjEtMzYuOSAxNC41IDYuNC00LjUgMTUuNy04LjIgMjcuMy01ek0yOTkuNCAxODYuOGMtMjEuOS02LjItMzUuMyAxMi4xLTM2LjkgMTQuNSA2LjQtNC41IDE1LjctOC4yIDI3LjMtNSAxMS45IDMuNCAxOC4yIDExLjcgMjEuNSAxOS4xbDEzLjMgMy44cy0xLjktMjUuOC0yNS4yLTMyLjR6Ii8%2BCjxwYXRoIGZpbGw9IiNFMjU3NEMiIGQ9Ik0xNjEuNyAyMjAuMXYtOTJoMzEuMmMtMy40LTEwLjUtNi43LTE4LjYtOS41LTI0LjItNC42LTkuMy05LjMtMy4xLTE5LjkgNS44LTcuNSA2LjMtMjYuNCAxOS42LTU0LjkgMjcuMy0yOC41IDcuNy01MS41IDUuNi02MS4xIDMuOS0xMy42LTIuNC0yMC44LTUuNC0yMCA1IC42IDkuMSAyLjggMjMuMyA3LjcgNDIuMSAxMC44IDQwLjUgNDYuNCAxMTguNiAxMTMuOCAxMDAuNSAxNy42LTQuNyAzMC0xNC4xIDM4LjYtMjYuMWgtMjUuOXYtMjIuNWwtNjIuNCAxNy43czQuNi0yNi44IDM3LjEtMzZjOS45LTIuOCAxOC40LTIuOCAyNS4zLTEuNXoiLz4KPHBhdGggZmlsbD0iI0Q2NTM0OCIgZD0iTTEzOS45IDI0NmwtNDAuNiAxMS41czQuNC0yNS4xIDM0LjMtMzVsLTIyLjktODYuMi0yIC42Yy0yOC41IDcuNy01MS41IDUuNi02MS4xIDMuOS0xMy42LTIuNC0yMC44LTUuNC0yMCA1IC42IDkuMSAyLjggMjMuMyA3LjcgNDIuMSAxMC44IDQwLjUgNDYuNCAxMTguNiAxMTMuOCAxMDAuNWwyLS42eiIvPgo8cGF0aCBmaWxsPSIjMkQ0NTUyIiBkPSJNMTM2LjQgMjIxLjZjLTEyLjkgMy43LTIxLjMgMTAuMS0yNi45IDE2LjUgNS4zLTQuNyAxMi41LTkgMjIuMS0xMS43IDkuOS0yLjggMTguMy0yLjggMjUuMi0xLjR2LTUuNGMtNS45LS41LTEyLjctLjItMjAuNCAyek0xMDguOSAxNzUuOWwtNDcuOCAxMi42czEwLjYgMTUuMyAyOC41IDEwLjVjMTcuOS00LjcgMTkuMy0yMy4xIDE5LjMtMjMuMXoiLz4KPC9zdmc%2B&logoColor=white)](https://github.com/seamus-sloan/omnibus/actions/workflows/e2e.yml)
[![CSS Lint](https://img.shields.io/github/actions/workflow/status/seamus-sloan/omnibus/css-lint.yml?branch=main&label=CSS%20Lint&logo=css)](https://github.com/seamus-sloan/omnibus/actions/workflows/css-lint.yml)
[![codecov](https://img.shields.io/codecov/c/github/seamus-sloan/omnibus?branch=main&logo=codecov&logoColor=white&label=coverage)](https://codecov.io/gh/seamus-sloan/omnibus)
[![Docker Hub](https://img.shields.io/docker/v/sesloan/omnibus?sort=semver&logo=docker&logoColor=white&label=docker%20hub&color=2496ED)](https://hub.docker.com/r/sesloan/omnibus/tags)
[![Image size](https://img.shields.io/docker/image-size/sesloan/omnibus?sort=semver&logo=docker&logoColor=white&label=image&color=2496ED)](https://hub.docker.com/r/sesloan/omnibus/tags)
[![Docker pulls](https://img.shields.io/docker/pulls/sesloan/omnibus?logo=docker&logoColor=white&color=2496ED)](https://hub.docker.com/r/sesloan/omnibus)

</div>

<!-- Hero shot: swap this for the wide screenshot. -->
<div align="center">
<img src="docs/screenshots/library.png" alt="Omnibus library — cover grid" width="100%" />
</div>

### [**→ Take the tour at seamus-sloan.github.io/omnibus**](https://seamus-sloan.github.io/omnibus/)

Cover-art grid or dense metadata table, smart shelves, an in-browser EPUB
reader, an HLS audiobook player, Kobo and Kindle delivery, physical-copy
tracking, and a native iOS app — all over one SQLite database and your own
files, untouched. The [site](https://seamus-sloan.github.io/omnibus/) has the
screenshots and the full feature tour.

> [!NOTE]
> **This is in active development.** Foundations and browse/discovery have
> shipped; reading/listening is in progress. See the
> [roadmap](https://github.com/users/seamus-sloan/projects/2/views/9) for
> what's next.

## Quick start

The image is published to
[Docker Hub as `sesloan/omnibus`](https://hub.docker.com/r/sesloan/omnibus/tags),
and the repo ships a Jellyfin-style [`docker-compose.yml`](docker-compose.yml).

```bash
# 1. Point the library mounts at your books and set your access URL.
$EDITOR docker-compose.yml

# 2. Build the bundle and start it (first build compiles the workspace + WASM).
docker compose up -d --build

# 3. Open http://localhost:3000 and register — the first account is the admin.
```

Volumes, env vars, reverse-proxy/TLS, PUID/PGID, and admin recovery are all in
the [Docker guide](docs/docker.md).

## Documentation

| | |
|---|---|
| [**Feature tour**](https://seamus-sloan.github.io/omnibus/) | What Omnibus does, with screenshots |
| [**Deploy with Docker**](docs/docker.md) | Volumes, env vars, TLS, PUID/PGID, admin recovery |
| [**Kobo sync & KEPUB**](docs/kobo.md) | Wired one-click transfer, wireless sync setup, two-way highlights |
| [**Local development**](docs/local-development.md) | Nix shells, `just` recipes, tests, mobile builds, project layout |
| [**Architecture**](docs/architecture.md) | Crate and module maps, request flows, mobile auth |
| [**Configuration reference**](.env.example) | Every supported environment variable, annotated |

## License

[MIT](LICENSE).
