---
name: add-backend-route
description: Recipe for adding a backend route to omnibus — a Dioxus server function (web-facing RPC) or a hand-written `/api/*` REST endpoint (mobile-facing). Triggers when the user asks to add a new endpoint, handler, server function, or fullstack feature.
---

# Add a backend route

Omnibus is a Dioxus fullstack app with two parallel transport layers. Pick the right one first:

| Client | Transport | Path convention | Lives in |
|---|---|---|---|
| **Web (WASM)** | Dioxus server function — `#[get]` / `#[post]` macro | `/api/rpc/<name>` | [frontend/src/rpc/](../../../frontend/src/rpc/) (per-domain submodule) |
| **Mobile (Dioxus Native)** | Hand-written axum handler called via `reqwest` | `/api/<resource>` | [server/src/backend.rs](../../../server/src/backend.rs) |

A new user-facing feature typically needs **both** (mobile+web parity), since the components in `frontend/src/pages/` drive both targets through `frontend/src/data.rs`.

## 1. Decide the route shape

- **New page route:** extend the `Route` enum in [frontend/src/lib.rs](../../../frontend/src/lib.rs) and add a page component under `frontend/src/pages/`. Dioxus fullstack handles SSR + hydration automatically — no new handler required.
- **Data-fetching endpoint:** see the two-transport table above. Add a server function **and** a REST handler, or one of them if the feature is platform-specific.
- **Binary upload (file/multipart):** server functions JSON-serialize their args, so they can't carry a file. Use a hand-written `/api/*` REST handler with `axum::extract::Multipart` for **both** web and mobile, and have the web `data/` helper POST directly via `gloo-net` + `web_sys::FormData` (see `backend/uploads.rs` + `data/uploads.rs`, modeled on the author-photo upload). Large uploads need their own sub-router with a bigger `DefaultBodyLimit`; if the handler then triggers a reindex, `worker.post(Task::Scan{..})` + `await_completion` before reading the result back.

## 2. Add the shared request/response types

Pick the right submodule under `shared/src/` ([lib.rs](../../../shared/src/lib.rs) is just a re-export index):
`settings.rs` (library paths), `ebook/` (book metadata + overrides), `discovery.rs` (browse / search-palette / author / series), `progress.rs` (F2.1 reading/listening), `audiobook.rs` (F2.3 manifest), `highlight.rs` (annotation colors + CRUD payloads), `view_prefs.rs` (landing-page sort/filter state), `auth.rs` (login/register payloads), `worker.rs` (worker progress feed). If the new type doesn't fit any of those, add a new submodule alongside them and re-export from `lib.rs`.

- Define the request/response bodies with `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`. Add `Eq` when every field supports it (skip it on any payload that carries `f64` or another non-`Eq` type).
- Keep this crate dioxus-free — pure serde only.
- The flat re-exports in `lib.rs` mean callers keep writing `omnibus_shared::Foo` regardless of which submodule `Foo` lives in.

## 3. Add the server function (web transport)

In the matching per-domain submodule under [frontend/src/rpc/](../../../frontend/src/rpc/) (e.g. `rpc/settings.rs`, `rpc/books.rs`), or a new one added to `rpc/mod.rs`:

```rust
#[post("/api/rpc/my_action", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_my_action(input: MyInput) -> Result<MyOutput> {
    let result = db::do_work(&pool.0, &input).await?;
    Ok(result)
}
```

- The server-only extractors are declared after the path in the macro. `pool: PoolExt` gives the SQLite pool. `_user: AuthUser` (or `_admin: AdminUser` for state-changing ops on shared config) enforces per-route authorization (F0.7) — both extractors live in `mod server_auth` in `rpc/mod.rs` and are imported into each submodule via `use super::{AuthUser, AdminUser, PoolExt, WorkerExt};` (gated on `feature = "server"`). The leading `_` is intentional until the route actually consumes the user (per-user data lands with F2.1+).
- `Result<T>` is the anyhow-backed alias from `dioxus::prelude::Result`. Domain errors use `thiserror` per [02-error-handling.md](../../rules/02-error-handling.md).
- The function body is only compiled when `feature = "server"` is active — guard any other imports with `#[cfg(feature = "server")]`. At the top of the submodule, import the DB layer as `use omnibus_db as db;` (gated on `feature = "server"`; add `scanner` when needed). Background reindex work goes through the shared `Worker` extension (`worker: WorkerExt` on the macro, then `worker.0.post(omnibus_db::worker::Task::Scan { library_path })`) — never `tokio::spawn(indexer::reindex(...))` from a handler.
- Dioxus auto-registers the route via `dioxus::server::router(App)` in [server/src/main.rs](../../../server/src/main.rs) — no manual registration.

## 4. Add the hand-written REST handler (mobile transport)

In [server/src/backend.rs](../../../server/src/backend.rs):

- Register on `rest_router()` with `.route(...)`.
- **First argument is the auth extractor** (`_user: AuthUser` for read paths and per-user mutations, `_admin: AdminUser` for shared-config writes). Then `State<AppState>` for the pool, `Json<T>` / `Path<…>` / `Query<…>` for the rest. F0.7 makes this mandatory — without an extractor the route would default to "any logged-in user" which defeats the per-user permission columns.
- **Image-serving GET whose bytes go into an `<img src>`** (covers/thumbs/photos): use `_user: MediaAuthUser` instead of `AuthUser`, and add the path prefix to `is_media_read_path` in `auth/gate.rs`. Both accept the session as a `?token=` query param — a mobile WebView's `<img>` fetch carries neither the bearer header nor a cookie. On the frontend build the URL via `crate::media_url` / `crate::thumb_url` (in `contexts.rs`) so the token is appended per URL, including each `srcset` candidate.
- **GET that streams bytes off disk** (a book file, an audio part, a download): route it through `backend::conditional` so it publishes an `ETag`, answers `If-None-Match` with a 304, and refuses a `Range` whose `If-Range` went stale. `tower-http`'s `ServeFile` gives you an ETag but has **no `If-Range` support**, so delegating to it alone lets a resume splice the tail of a new file onto the head of an old one. See [09-content-validators.md](../../rules/09-content-validators.md) for the call shape and why `ETag` and `Vary` must travel together.
- Pick a URL under `/api/<resource>` that does **not** collide with the `/api/rpc/*` namespace used by server functions.
- Return `Response` with explicit status + error string on failure so mobile's error UI can surface it.

## 5. Add the DB query (if needed)

In the matching domain module under [db/src/](../../../db/src/) (`books/`, `progress.rs`, `settings/`, a new sibling module for a new domain — `queries.rs` was split apart):

- Define a typed error variant in the module's error enum (or add one) per [02-error-handling.md](../../rules/02-error-handling.md).
- Schema changes go as a new numbered SQL file under [db/migrations/](../../../db/migrations/) (never edit an applied file). Re-exported from `omnibus_db::` (see the flatten block in `db/src/lib.rs`) so callsites just write `omnibus_db::my_query(...)`.
- If the new table interacts with book identity, mind `merged_uuids` (cross-format attach/merge, migration 0016): book-keyed reads should resolve uuids via `resolve_book_id_by_uuid`, which falls back to merged uuids.

## 6. Wire the unified data layer

In [frontend/src/data.rs](../../../frontend/src/data.rs) (per-domain files under `data/`), add a function that the page component calls, with both transport implementations:

- `#[cfg(feature = "mobile")]` — builds `reqwest` call to `/api/<resource>`.
- `#[cfg(not(feature = "mobile"))]` — calls the server function from `crate::rpc`.

The page component then calls a single `data::my_action(...)` and works on both targets.

**Mobile offline layer.** Mobile data fns are expected to participate in the offline-first layer ([frontend/src/offline.rs](../../../frontend/src/offline.rs)):

- **Reads**: keep the raw REST body in a `pub(crate) *_online` fn and make the public fn a `crate::offline::cache::read_through(key, async move { *_online(...).await })` wrapper (the future needs owned args — `Send + 'static`), with the key added to `offline::cache::keys`. The policy is cache-first (stale-while-revalidate): cached copy served instantly, background revalidation bumps the generation channel pages watch via `use_cache_generation`. Use `read_through_network_first` only when acting on a stale answer would be a correctness bug (see `get_me` / `get_progress` for the two existing cases). See any read in `data/books.rs`.
- **User-scoped writes**: same `*_online` split, with the public fn going through `crate::offline::sync::write_through(online, queue)` plus a matching `queue_*` fn in `offline::outbox` (an `Op` variant, a coalesce key if it's an idempotent upsert, an optimistic cache apply in `outbox/apply.rs`) and a replay arm in `offline::sync::execute_op`. See `data/ratings.rs` for the smallest full example.
- **Admin/server-only endpoints** (settings, scan, SMTP, uploads) stay plain online-only fns — they intentionally fail fast offline.

## 7. Add tests

Per [03-unit-testing.md](../../rules/03-unit-testing.md):

- **DB:** sibling `<mod>/tests.rs` in the relevant module (inline `#[cfg(test)]` only for 1-2 trivial cases). Happy path + not-found + constraint violation. Run with `cargo test -p omnibus-db`.
- **REST handler:** sibling `<module>/tests.rs` next to the handler module (e.g. `server/src/backend/progress/tests.rs`); inline `#[cfg(test)]` only for routes still in `server/src/backend.rs` itself. Drive with `tower::ServiceExt::oneshot` against `rest_router(AppState::new(in-memory pool))`. Bootstrap a session via the helpers in [server/src/auth/test_support.rs](../../../server/src/auth/test_support.rs) (`create_user` / `create_admin` / `bearer_token`) and attach the bearer header. Cover the full matrix per [03-unit-testing.md](../../rules/03-unit-testing.md): 200 (authed) + 401 (anon) + 403 (wrong role, for admin-gated routes) + relevant 4xx/5xx. Run with `cargo test -p omnibus`.
- **Server function:** covered indirectly by the DB tests (the function body is a thin wrapper). Add an integration test only if the wrapper does non-trivial composition.

## 8. Add Playwright coverage (user-facing changes)

See [add-playwright-flow](../add-playwright-flow/SKILL.md). Use the `/api/rpc/*` URL in `expectMutation` for web tests — Playwright drives the browser, which calls server functions, not REST.

## 9. Verify in the browser (when the route surfaces in the UI)

If this route is hit by a UI page, drive that page via [`ui-validate`](../ui-validate/SKILL.md) — it brings up the dev server (idempotent, identity-checked across `jj` workspaces), logs in as the seeded admin, and uses Claude Preview (`mcp__Claude_Preview__preview_*`) to snapshot the new behavior.

## 10. End-of-session

Run [99-end-of-session.md](../../rules/99-end-of-session.md). If a new module was introduced, update [docs/architecture.md](../../../docs/architecture.md) (per-crate module maps), and add a link to it from [CLAUDE.md](../../../CLAUDE.md) if it's a top-level crate or concept worth listing in the index.
