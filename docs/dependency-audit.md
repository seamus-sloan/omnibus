# Dependency Audit — Cargo.lock Duplicate Families

Last updated: 2026-08-09 (issue #1767; previously issues #1710, #1621, #1529, #1530, #1100, #1268, #1147, #643).

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
| `getrandom` | 0.1.16, 0.2.17, 0.3.4, 0.4.2 | 0.1 from `rand 0.7.3` (via `phf_generator 0.8.0`, build-dep only); 0.2 from our `db` (argon2/rand_core 0.6 auth path) + ring + sqlx; 0.3 from rand 0.9 (tungstenite transitive); 0.4 is a **runtime** dependency via `uuid`'s `v4` feature (`db::helpers::mint_uuid` calls `Uuid::new_v4()` in non-test code) plus `cfb`/`infer` under `dioxus-asset-resolver` — `tempfile` also pulls in the same 0.4 line, but only as a separate dev-only consumer | **accepted intentionally** | The 0.2 pin in `db` is deliberate: `rand_core@0.6` + `getrandom@0.2` is the stable API surface for `OsRng::generate` in the Argon2 password hashing path (see `db/Cargo.toml` comment). Bumping to 0.3 would require also bumping `rand_core`, `password-hash`, and `argon2` — a non-trivial auth-layer upgrade. `getrandom@0.4` links into the shipped server binary via `uuid`'s `v4` feature (and `cfb`/`infer`); `tempfile` is a separate, dev-only consumer of the same version. `getrandom@0.1` is build-only (`phf_codegen`/`string_cache_codegen` chain) and never links into the runtime binary. |
| `rand_core` | 0.5.1, 0.6.4, 0.9.5 | 0.5.1 via `rand 0.7.3` (build-dep, `phf_generator`); 0.6.4 via argon2 auth path + signature crates; 0.9.5 via `rand 0.9` (tungstenite) | **blocked by upstream** | Three-way split mirrors the `rand` family below. 0.5/0.6 are the auth and build-dep paths described elsewhere; 0.9 is the new tungstenite path. |
| `hashbrown` | 0.14.5, 0.15.5, 0.16.1, 0.17.1 | 0.14 from `dashmap` (dioxus-server); 0.15 from `sqlx-core` + `hashlink`; 0.16 from `lru` (dioxus-server); 0.17 from `indexmap` (h2 + sqlx + epub/zip) | **blocked by upstream** | Four-way spread across two major upstream stacks (Dioxus + sqlx + h2). All versions are internal impl details of their respective crates — `hashbrown` does not appear in any public API. Collapses as each upstream crate independently catches up to the same `hashbrown` minor. |
| `foldhash` | 0.1.5, 0.2.0 | 0.1.5 via `hashbrown 0.15.5` (sqlx-core/hashlink path); 0.2.0 via `hashbrown 0.16.1` (dioxus-server's `lru`/`metrics-util` path) | **accepted intentionally** | One copy per `hashbrown` minor — both are internal impl details of `hashbrown`, same root cause as the `hashbrown` row above. Classified separately, though: the `deny.toml` skips for both `foldhash` versions are already in place, unlike `hashbrown`'s still-open four-way spread. Already carries a `deny.toml` skip + comment. |
| `hashlink` | 0.9.1, 0.10.0 | 0.9.1 via `rusqlite 0.32.1` (`omnibus-frontend`'s mobile-offline SQLite store); 0.10.0 via `sqlx-core 0.8.6` | **blocked by upstream** | Downstream instance of the same `rusqlite`/`sqlx` `libsqlite3-sys` coupling already documented for issue #1529 (see Policy section below) — the two crates simply vendor different `hashlink` minors alongside their independent `libsqlite3-sys` pins. Collapses only when that coordinated `rusqlite`/`sqlx` bump happens. |
| `convert_case` | 0.4.0, 0.8.0, 0.10.0 | 0.4 from `derive_more 0.99.20` (proc-macro path, transitive via Dioxus desktop/mobile shells); 0.8 from Dioxus proc-macro crates (`dioxus-core-macro`, `dioxus-html-internal-macro`, `dioxus-stores-macro`); 0.10 from `derive_more-impl` (pulled by `dioxus-fullstack`) | **blocked by upstream** | Pure proc-macro dependency; not in any runtime binary image. |
| `rand` | 0.7.3, 0.8.6, 0.9.4 | 0.7 from `phf_generator 0.8.0` (build dep of `ammonia` → `html5ever` chain); 0.8 from a separate `phf_generator` path; 0.9 from `rand_core@0.9` (via tungstenite) | **blocked by upstream** | The 0.7 and 0.8 copies are build-only (`phf_codegen` / `string_cache_codegen`) and never link into the runtime binary. |
| `webpki-roots` | 0.26.11, 1.0.7 | 0.26 from `sqlx-core`; 1.0.7 from `hyper-rustls` (reqwest) | **blocked by upstream** | sqlx-core pins 0.26; reqwest/hyper-rustls have moved to 1.x. Collapses when sqlx bumps its rustls deps. |
| `axum-extra` | 0.10.3, 0.12.6 | 0.10.3 via `dioxus-fullstack` 0.7.9 (the git-pinned Dioxus tag); 0.12.6 via our own `omnibus` server crate directly | **blocked by upstream** | Mirrors the `tower-http` split below: the Dioxus git pin hasn't caught up to the `axum-extra` version our server crate uses. Collapses on the next Dioxus bump. |
| `tower-http` | 0.6.11, 0.7.0 | 0.6.11 via `dioxus-fullstack`/`dioxus-server` 0.7.9 and transitively via **both** `reqwest` 0.12.28 and 0.13.4 (each vendors its own `tower-http` dependency); 0.7.0 via our own `omnibus` server crate directly | **blocked by upstream** | Not purely a Dioxus-version-lag issue like `axum-extra` above — `reqwest` 0.13.4 (our own workspace-pinned version, not just the older `dioxus-fullstack` copy) also depends directly on `tower-http 0.6.11` (see `Cargo.lock`'s `reqwest 0.13.4` dependency block). So a Dioxus bump alone won't collapse this: it persists until `reqwest` itself upgrades to `tower-http 0.7`, or our own server crate's direct `tower-http` dep is downgraded to 0.6.11 to match. |
| `reqwest` | 0.12.28, 0.13.4 | 0.12.28 via `dioxus-fullstack` 0.7.9 (the git-pinned Dioxus tag); 0.13.4 via `omnibus-db`, `omnibus-frontend`, `omnibus-mobile` — the workspace-pinned version per `Cargo.toml`'s `[workspace.dependencies] reqwest = { version = "0.13", ... }` | **blocked by upstream** | Two-*major*-version split, the largest gap in this table (the rest is minor-version churn). Exists because `dioxus-fullstack`'s git-pinned tag hasn't caught up to the `reqwest 0.13` the workspace standardized on. Should collapse on the next Dioxus bump, same caveat as the `tungstenite`/`const-serialize` rows. |
| `gloo-net` | 0.6.0, 0.7.0 | 0.6.0 via `dioxus-fullstack` 0.7.9 (the git-pinned Dioxus tag); 0.7.0 via `omnibus-frontend`'s own direct pin (`frontend/Cargo.toml`, behind the `web` feature) | **blocked by upstream** | Same "Dioxus git-pin lag" shape as `axum-extra`/`tower-http`/`reqwest` above: the git-pinned Dioxus tag hasn't caught up to the `gloo-net` version the WASM frontend pins directly. Resolves automatically once Dioxus publishes a crates.io release matching the git tag, tracked by #522. |
| `gloo-timers` | 0.3.0, 0.4.0 | 0.3.0 via `dioxus-web` 0.7.9 (the git-pinned Dioxus tag); 0.4.0 via `omnibus-frontend`'s own direct pin (`frontend/Cargo.toml`, behind the `web` feature) | **blocked by upstream** | Same root cause as the `gloo-net` row above. Resolves automatically once Dioxus publishes a crates.io release matching the git tag, tracked by #522. |
| `gloo-utils` | 0.2.0, 0.3.0 | 0.2.0 transitively via `gloo-net 0.6.0`; 0.3.0 transitively via `gloo-net 0.7.0` | **blocked by upstream** | Follows the `gloo-net` split one level down and collapses in lockstep with it. Resolves automatically once Dioxus publishes a crates.io release matching the git tag, tracked by #522. |
| `digest` | 0.10.7 (×2) | Both consumers are `sha1` (axum/tungstenite) + `blake2`/`sha2` (argon2/sqlx) | N/A — same version | `cargo tree -d` shows two entry paths to the same version (different consumers). No actual duplicate in the lockfile. |
| `rustc-hash` | 1.1.0, 2.1.2 | 1.1.0 via `sledgehammer_utils` (pulled in by `dioxus-interpreter-js`, which both `dioxus-web` and `dioxus-server` depend on); 2.1.2 as a direct dependency of `dioxus-core` | **blocked by upstream** | Verified with `cargo tree -i rustc-hash@1.1.0` / `@2.1.2` (not assumed): both versions are reachable from the default, non-mobile build via `dioxus-core`/`dioxus-server`/`dioxus-web`, which `omnibus` and `omnibus-frontend` depend on directly — i.e. **inside** the shipped web/WASM bundle, not confined to the `wry`/`tao` mobile native-shell subtree (despite that subtree's skip-tree comment in `deny.toml` also naming `rustc-hash` among its duplicates — the mobile subtree apparently resolves to one of these same two versions rather than introducing a third). Collapses when Dioxus unifies its internal `rustc-hash` pin. |
| `bytes`, `futures-*`, `num-traits`, `tokio`, `manganis-core`, `log`, `fastrand`, `sqlx-sqlite` | (shown ×2 each) | Multiple downstream consumers | N/A — same version | Same pattern as `digest`: one version, multiple reverse-dependency entry points in the `cargo tree -d` output. Not true duplicates. |

## First-party skew resolved in this PR

- `thiserror` in `db/Cargo.toml` and `frontend/Cargo.toml` aligned to `thiserror.workspace = true` (workspace declares `thiserror = "2"`). Previously both crates pinned `thiserror = "1"` independently. The workspace now acts as a single source of truth — new crates must use `.workspace = true`.

## Accepted advisories

- `lru 0.16.4` — RUSTSEC-2026-0253 (unsound: use-after-free in
  `LruCache::pop()` under a panicking `Drop`). Reaches the production graph
  via the git-pinned `dioxus-server` → `dioxus` (see #522 for the pin
  itself); not independently bumpable. Accepted — exploiting it requires
  `catch_unwind` plus a panicking `Drop` on a cached key during a pop, which
  `dioxus-server`'s internal use does not trigger. `deny.toml` ignores it
  with a matching comment. Revisit when Dioxus bumps `lru` to >= 0.18.2.

## Yanked crates

- `spin 0.9.8` — flagged by `cargo audit` as yanked from crates.io (no
  RUSTSEC advisory, 0 vulnerabilities). Reaches the production graph two
  ways: `sqlx-sqlite → flume → spin` and `axum → multer → spin`. Accepted
  until `flume`/`multer` naturally drop it on their next releases — it is a
  yanked-not-vulnerable notice, and both parents are well-maintained.
  `deny.toml` sets `[advisories] yanked = "warn"` with a matching comment.

## Policy

- **Plain version-currency pins** (issue #1529): a dependency frozen behind
  current stable earns a `# PIN RATIONALE: ...` comment at its pin site even
  when it isn't part of a duplicate-family split — the same "explain, don't
  silently drift" principle this doc has always applied to duplicates, now
  extended to any deliberately-stale single-version pin (`zip`, `sqlx`,
  `rusqlite`, `argon2`, etc.). State *why* the freeze is intentional and
  *what* would need to change to lift it, so the next person to touch that
  line doesn't have to reconstruct the reasoning from a git-blame.
- **Adding a new crate:** check `cargo tree -d` after `cargo update`; any new duplicate must be classified here before the PR lands.
- **Bumping Dioxus:** re-run this audit. The websocket and const-serialize clusters will be the first to collapse.
- **Promoting `getrandom`:** when `argon2`/`password-hash` ship a version that supports `getrandom@0.3`, consolidate the 0.2 + 0.3 entries. Until then the three-way split is intentional.
- **Bumping `sqlx`:** held on the 0.8 line (`Cargo.toml` allows 0.8.x patch bumps; upstream has published 0.9.0); revisit as a coordinated bump when the `rusqlite`/`sqlx` `libsqlite3-sys` coupling from issue #1529 needs to move together, same trigger shape as the `getrandom` note above.
- **`deny.toml` skip entries:** every blocked/accepted duplicate has a matching `skip` entry in the `[bans]` section of `deny.toml` so `cargo deny check bans` does not regress on already-triaged duplicates.
