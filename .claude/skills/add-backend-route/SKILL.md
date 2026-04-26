---
name: add-backend-route
description: Recipe for adding a backend route to omnibus — a Dioxus server function (web-facing RPC) or a hand-written `/api/*` REST endpoint (mobile-facing). Triggers when the user asks to add a new endpoint, handler, server function, or fullstack feature.
---

# Add a backend route

Omnibus is a Dioxus fullstack app with two parallel transport layers. Pick the right one first:

| Client | Transport | Path convention | Lives in |
|---|---|---|---|
| **Web (WASM)** | Dioxus server function — `#[get]` / `#[post]` macro | `/api/rpc/<name>` | [frontend/src/rpc.rs](../../../frontend/src/rpc.rs) |
| **Mobile (Dioxus Native)** | Hand-written axum handler called via `reqwest` | `/api/<resource>` | [server/src/backend.rs](../../../server/src/backend.rs) |

A new user-facing feature typically needs **both** (mobile+web parity), since the components in `frontend/src/pages/` drive both targets through `frontend/src/data.rs`.

## 1. Decide the route shape

- **New page route:** extend the `Route` enum in [frontend/src/lib.rs](../../../frontend/src/lib.rs) and add a page component under `frontend/src/pages/`. Dioxus fullstack handles SSR + hydration automatically — no new handler required.
- **Data-fetching endpoint:** see the two-transport table above. Add a server function **and** a REST handler, or one of them if the feature is platform-specific.

## 2. Add the shared request/response types

In [shared/src/lib.rs](../../../shared/src/lib.rs):

- Define the request/response bodies with `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]`.
- Keep this crate dioxus-free — pure serde only.

## 3. Add the server function (web transport)

In [frontend/src/rpc.rs](../../../frontend/src/rpc.rs):

```rust
#[post("/api/rpc/my_action", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_my_action(input: MyInput) -> Result<MyOutput> {
    let result = db::do_work(&pool.0, &input).await?;
    Ok(result)
}
```

- The server-only extractors are declared after the path in the macro. `pool: PoolExt` gives the SQLite pool. `_user: AuthUser` (or `_admin: AdminUser` for state-changing ops on shared config) enforces per-route authorization (F0.7) — both extractors live in `mod server_auth` at the top of `rpc.rs`. The leading `_` is intentional until the route actually consumes the user (per-user data lands with F2.1+).
- `Result<T>` is the anyhow-backed alias from `dioxus::prelude::Result`. Domain errors use `thiserror` per [02-error-handling.md](../../rules/02-error-handling.md).
- The function body is only compiled when `feature = "server"` is active — guard any other imports with `#[cfg(feature = "server")]`. At the top of `rpc.rs`, import the DB layer as `use omnibus_db::{self as db, scanner};` (gated on `feature = "server"`). Background reindex work goes through the shared `Worker` extension (`worker: WorkerExt` on the macro, then `worker.0.post(omnibus_db::worker::Task::Scan { library_path })`) — never `tokio::spawn(indexer::reindex(...))` from a handler.
- Dioxus auto-registers the route via `dioxus::server::router(App)` in [server/src/main.rs](../../../server/src/main.rs) — no manual registration.

## 4. Add the hand-written REST handler (mobile transport)

In [server/src/backend.rs](../../../server/src/backend.rs):

- Register on `rest_router()` with `.route(...)`.
- **First argument is the auth extractor** (`_user: AuthUser` for read paths and per-user mutations, `_admin: AdminUser` for shared-config writes). Then `State<AppState>` for the pool, `Json<T>` / `Path<…>` / `Query<…>` for the rest. F0.7 makes this mandatory — without an extractor the route would default to "any logged-in user" which defeats the per-user permission columns.
- Pick a URL under `/api/<resource>` that does **not** collide with the `/api/rpc/*` namespace used by server functions.
- Return `Response` with explicit status + error string on failure so mobile's error UI can surface it.

## 5. Add the DB query (if needed)

In [db/src/queries.rs](../../../db/src/queries.rs):

- Define a typed error variant in a `DbError` enum (or add one) per [02-error-handling.md](../../rules/02-error-handling.md).
- Use `sqlx::query_as!` / `sqlx::query!` for compile-time checking against `DATABASE_URL`.
- Schema changes go as a new numbered SQL file under [db/migrations/](../../../db/migrations/) (never edit an applied file). Re-exported from `omnibus_db::` so callsites just write `omnibus_db::my_query(...)`.

## 6. Wire the unified data layer

In [frontend/src/data.rs](../../../frontend/src/data.rs), add a function that the page component calls, with both transport implementations:

- `#[cfg(feature = "mobile")]` — builds `reqwest` call to `/api/<resource>`.
- `#[cfg(not(feature = "mobile"))]` — calls the server function from `crate::rpc`.

The page component then calls a single `data::my_action(...)` and works on both targets.

## 7. Add tests

Per [03-unit-testing.md](../../rules/03-unit-testing.md):

- **DB:** inline `#[cfg(test)]` in `db/src/queries.rs` (or the relevant module). Happy path + not-found + constraint violation. Run with `cargo test -p omnibus-db`.
- **REST handler:** inline `#[cfg(test)]` in `server/src/backend.rs`. Drive with `tower::ServiceExt::oneshot` against `rest_router(AppState::new(in-memory pool))`. Bootstrap a session via the helpers in [server/src/auth/test_support.rs](../../../server/src/auth/test_support.rs) (`create_user` / `create_admin` / `bearer_token`) and attach the bearer header. Cover the full matrix per [03-unit-testing.md](../../rules/03-unit-testing.md): 200 (authed) + 401 (anon) + 403 (wrong role, for admin-gated routes) + relevant 4xx/5xx. Run with `cargo test -p omnibus`.
- **Server function:** covered indirectly by the DB tests (the function body is a thin wrapper). Add an integration test only if the wrapper does non-trivial composition.

## 8. Add Playwright coverage (user-facing changes)

See [add-playwright-flow](../add-playwright-flow/SKILL.md). Use the `/api/rpc/*` URL in `expectMutation` for web tests — Playwright drives the browser, which calls server functions, not REST.

## 9. End-of-session

Run [99-end-of-session.md](../../rules/99-end-of-session.md). Update [CLAUDE.md](../../../CLAUDE.md) module-structure section if a new module was introduced.
