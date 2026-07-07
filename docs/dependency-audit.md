# Dependency Audit — Cargo.lock Duplicate Families

Last updated: 2026-07-07 (issue #774).

Run `cargo tree -d` and inspect the output any time Dioxus or Axum are bumped.
The `deny.toml` `[bans]` section has matching `skip` entries for all accepted
and blocked duplicates; `cargo deny check bans` will warn on anything outside
this list.

## Duplicate-family inventory

| Crate | Versions | Sources | Classification | Notes |
|---|---|---|---|---|
| `tungstenite` | 0.27.0, 0.28.0, 0.29.0 | 0.27 via `async-tungstenite` (dioxus-fullstack); 0.28 via `dioxus-devtools` + `tokio-tungstenite@0.28`; 0.29 via `axum` (tokio-tungstenite@0.29) | **blocked by upstream** | Three-way split between Axum (latest) and two older Dioxus websocket paths. Collapses when Dioxus updates its dioxus-devtools/async-tungstenite deps. |
| `tokio-tungstenite` | 0.28.0, 0.29.0 | 0.28 via `dioxus-fullstack`, `dioxus-server`; 0.29 via `axum` | **blocked by upstream** | Same root cause as the tungstenite split above. |
| `thiserror` | 1.0.69, 2.0.18 | 1.0.69 via `cairo-rs`, `gio`, `glib`, `gloo-net`, `jni`, `ndk` (all transitive through the Dioxus WASM/desktop/mobile stacks); 2.0.18 via our crates + most of Dioxus | **blocked by upstream** | Our first-party crates (`omnibus-db`, `omnibus-frontend`) are on v2 via `thiserror.workspace = true`. The v1 copy comes only from upstream Dioxus dependencies we do not control. |
| `const-serialize` | 0.7.2, 0.8.0-alpha.0 | 0.7.2 from crates.io `manganis`; 0.8.0-alpha.0 from the Dioxus git pin | **blocked by upstream** | The pre-release alpha is bundled inside the Dioxus git tag. Cannot be collapsed until Dioxus publishes a stable release with a unified `const-serialize`. |
| `getrandom` | 0.1.16, 0.2.17, 0.3.4, 0.4.2 | 0.1 from `rand 0.7.3` (via `phf_generator 0.8.0`, build-dep only); 0.2 from our `db` (argon2/rand_core 0.6 auth path) + ring + sqlx; 0.3 from rand 0.9 (tungstenite transitive); 0.4 from `tempfile` (dev-dep only) | **accepted intentionally** | The 0.2 pin in `db` is deliberate: `rand_core@0.6` + `getrandom@0.2` is the stable API surface for `OsRng::generate` in the Argon2 password hashing path (see `db/Cargo.toml` comment). Bumping to 0.3 would require also bumping `rand_core`, `password-hash`, and `argon2` — a non-trivial auth-layer upgrade. `getrandom@0.4` is dev-only (tempfile). `getrandom@0.1` is build-only (`phf_codegen`/`string_cache_codegen` chain) and never links into the runtime binary. |
| `rand_core` | 0.5.1, 0.6.4, 0.9.5 | 0.5.1 via `rand 0.7.3` (build-dep, `phf_generator`); 0.6.4 via argon2 auth path + signature crates; 0.9.5 via `rand 0.9` (tungstenite) | **blocked by upstream** | Three-way split mirrors the `rand` family below. 0.5/0.6 are the auth and build-dep paths described elsewhere; 0.9 is the new tungstenite path. |
| `hashbrown` | 0.14.5, 0.15.5, 0.16.1, 0.17.1 | 0.14 from `dashmap` (dioxus-server); 0.15 from `sqlx-core` + `hashlink`; 0.16 from `lru` (dioxus-server); 0.17 from `indexmap` (h2 + sqlx + epub/zip) | **blocked by upstream** | Four-way spread across two major upstream stacks (Dioxus + sqlx + h2). All versions are internal impl details of their respective crates — `hashbrown` does not appear in any public API. Collapses as each upstream crate independently catches up to the same `hashbrown` minor. |
| `convert_case` | 0.4.0, 0.8.0, 0.10.0 | 0.4 from `derive_more 0.99.20` (proc-macro path, transitive via Dioxus desktop/mobile shells); 0.8 from Dioxus proc-macro crates (`dioxus-core-macro`, `dioxus-html-internal-macro`, `dioxus-stores-macro`); 0.10 from `derive_more-impl` (pulled by `dioxus-fullstack`) | **blocked by upstream** | Pure proc-macro dependency; not in any runtime binary image. |
| `rand` | 0.7.3, 0.8.6, 0.9.4 | 0.7 from `phf_generator 0.8.0` (build dep of `ammonia` → `html5ever` chain); 0.8 from a separate `phf_generator` path; 0.9 from `rand_core@0.9` (via tungstenite) | **blocked by upstream** | The 0.7 and 0.8 copies are build-only (`phf_codegen` / `string_cache_codegen`) and never link into the runtime binary. |
| `webpki-roots` | 0.26.11, 1.0.7 | 0.26 from `sqlx-core`; 1.0.7 from `hyper-rustls` (reqwest) | **blocked by upstream** | sqlx-core pins 0.26; reqwest/hyper-rustls have moved to 1.x. Collapses when sqlx bumps its rustls deps. |
| `reqwest` | 0.12.28, 0.13.4 | 0.12.28 via `dioxus-fullstack` (git-pinned Dioxus tag); 0.13.4 is our own direct dep (workspace + `db` + `frontend` mobile feature) | **blocked by upstream** | Bumped to 0.13 for issue #774; the 0.12 copy remains only because `dioxus-fullstack` depends on it directly. Collapses when Dioxus bumps its own `reqwest` past 0.13. |
| `axum-extra` | 0.10.3, 0.12.6 | 0.10.3 via `dioxus-server` (git-pinned Dioxus tag); 0.12.6 is our own direct dep in `server/Cargo.toml` | **blocked by upstream** | Bumped to 0.12 for issue #774. Collapses when Dioxus bumps its own `axum-extra` past 0.12. |
| `tower-http` | 0.6.11, 0.7.0 | 0.6.11 via `dioxus-fullstack`/`dioxus-server` (git-pinned Dioxus tag); 0.7.0 is our own direct dep in `server/Cargo.toml` | **blocked by upstream** | Bumped to 0.7 for issue #774. Collapses when Dioxus bumps its own `tower-http` past 0.7. |
| `digest` | 0.10.7 (×2) | Both consumers are `sha1` (axum/tungstenite) + `blake2`/`sha2` (argon2/sqlx) | N/A — same version | `cargo tree -d` shows two entry paths to the same version (different consumers). No actual duplicate in the lockfile. |
| `bytes`, `futures-*`, `num-traits`, `tokio`, `manganis-core` | (shown ×2 each) | Multiple downstream consumers | N/A — same version | Same pattern as `digest`: one version, multiple reverse-dependency entry points in the `cargo tree -d` output. Not true duplicates. |

## First-party skew resolved in this PR

- `thiserror` in `db/Cargo.toml` and `frontend/Cargo.toml` aligned to `thiserror.workspace = true` (workspace declares `thiserror = "2"`). Previously both crates pinned `thiserror = "1"` independently. The workspace now acts as a single source of truth — new crates must use `.workspace = true`.

## Policy

- **Adding a new crate:** check `cargo tree -d` after `cargo update`; any new duplicate must be classified here before the PR lands.
- **Bumping Dioxus:** re-run this audit. The websocket and const-serialize clusters will be the first to collapse.
- **Promoting `getrandom`:** when `argon2`/`password-hash` ship a version that supports `getrandom@0.3`, consolidate the 0.2 + 0.3 entries. Until then the three-way split is intentional.
- **`deny.toml` skip entries:** every blocked/accepted duplicate has a matching `skip` entry in the `[bans]` section of `deny.toml` so `cargo deny check bans` does not regress on already-triaged duplicates.
