# F0.7 — Per-route authorization

**Phase 0 · Foundations** · **Priority:** P0

Wire the existing `AuthUser` / `AdminUser` extractors onto every `/api/*` and `/api/rpc/*` handler so the permission columns landed in [F0.3](0-3-auth.md) are actually enforced.

## Objective

Today the [`require_auth` gate](../../server/src/auth/gate.rs) middleware admits any logged-in user to every protected route. The `is_admin` / `can_upload` / `can_edit` / `can_download` columns from F0.3 are surfaced via `/api/auth/me` but never consulted. Every handler in [server/src/backend.rs](../../server/src/backend.rs) and [frontend/src/rpc.rs](../../frontend/src/rpc.rs) takes only `State<AppState>` — the typed user is dropped on the floor. Goal: every protected handler declares the strictest extractor it needs (`AuthUser` for read paths, `AdminUser` for state-changing ops on shared config), and the request is rejected before it touches the data layer if the caller lacks the permission.

## User / business value

Closes a real privilege-escalation today: any non-admin who registers (or is added by an admin in F5.4) can `POST /api/settings` and rewrite the library root paths, or `POST /api/rpc/settings` and trigger an arbitrary reindex. Pre-requisite for shipping any feature that reads or writes per-user state — without enforcement, F2.1 progress sync, F3.2 ratings, F3.1 libraries all collapse into a single shared blob the moment more than one user exists.

## Technical considerations

- `AuthUser` extractor at [server/src/auth/extractor.rs:81](../../server/src/auth/extractor.rs:81) and `AdminUser` wrapper at [extractor.rs:111](../../server/src/auth/extractor.rs:111) already exist — the work is wiring, not building.
- Audit each route and pick the right extractor. First pass:
  - `GET /api/value`, `GET /api/library`, `GET /api/ebooks`, `GET /api/covers/{id}`, `GET /api/search` → `AuthUser`.
  - `POST /api/value/increment` → `AuthUser` (placeholder; will go away with the counter app).
  - `GET/POST /api/settings`, `POST /api/rpc/settings` → `AdminUser` (settings are server-global today).
- Same pass on the Dioxus server functions in [frontend/src/rpc.rs](../../frontend/src/rpc.rs). Server functions can extract from request parts via `extract::<AuthUser, _>().await` inside the body — server-function ergonomics, not handler args.
- When the schema gains per-user tables ([F0.1](0-1-schema-refactor.md) follow-ups, [F2.1 progress sync](2-1-progress-sync.md), etc.), the read/write helpers should take `user_id: i64` rather than reading global state — the extractor delivers it. This initiative does the wiring; data-layer per-user scoping rides along with the feature that introduces the table.
- Tests: every handler integration test in [server/src/backend.rs](../../server/src/backend.rs) currently calls `oneshot` without a session cookie. After this change, the test helpers need a `as_user(pool, &user)` / `as_admin(pool, &user)` shim that issues a session and attaches the cookie, and the existing assertions that rely on "logged-in by default" should split into 200 (authed) + 401 (anonymous) + 403 (wrong role) cases per [03-unit-testing.md](../../.claude/rules/03-unit-testing.md).

## Dependencies

- [F0.3 Auth](0-3-auth.md) — extractors, session table, permission columns. Already shipped.

## Risks

- Touches every protected handler. Diff is mechanical but wide; review burden is real.
- Tests get noisier — every "happy path" test now needs a session bootstrap. Worth investing in a single `tests::auth_helpers` module rather than copy-pasting per-file.

## TODOs

### Backend route audit

**What:** For every handler under [server/src/backend.rs](../../server/src/backend.rs), declare the appropriate extractor (`AuthUser` or `AdminUser`) and update the integration tests to bootstrap a session.

**Why:** Without this, the per-user permission columns are decorative.

**Effort:** M
**Priority:** P0
**Depends on:** None.

### RPC server-function audit

**What:** Same audit for every `#[get]` / `#[post]` in [frontend/src/rpc.rs](../../frontend/src/rpc.rs). Use `extract::<AuthUser, _>()` inside server-function bodies.

**Why:** Web clients reach the same data layer through `/api/rpc/*`; an audit that only covers `/api/*` leaves the web surface wide open.

**Effort:** S
**Priority:** P0
**Depends on:** Backend route audit (share the auth-helpers shim).

### Test helper for authed integration tests

**What:** Add a `server::auth::test_support` (or similar) module exposing `as_user(pool, username, password) -> CookieJar` and `as_admin(...)` so handler tests can attach a real session header in one line.

**Why:** Without a shared helper, every test file reinvents the bootstrap and drifts.

**Effort:** S
**Priority:** P0
**Depends on:** None.

## Status

Shipped 2026-04-26. Every protected handler in [server/src/backend.rs](../../server/src/backend.rs) now declares `AuthUser` (read paths + the placeholder counter mutation) or `AdminUser` (`/api/settings` GET/POST), and every Dioxus server function in [frontend/src/rpc.rs](../../frontend/src/rpc.rs) does the same. The `AuthUser` / `AdminUser` `FromRequestParts` impls were decoupled from `AppState` and now read the pool from `Extension<SqlitePool>` so the same logic backs both routers; the wire-format token resolution lives in [`omnibus_db::auth::parse_session_token`](../../db/src/auth.rs) so the `frontend` crate can reuse it without taking a dep on the `server` crate. Integration-test bootstrap is consolidated in [`server/src/auth/test_support.rs`](../../server/src/auth/test_support.rs); each protected route gained an anon-401 sibling test, and admin-only routes additionally gained a non-admin-403 sibling test per [03-unit-testing.md](../../.claude/rules/03-unit-testing.md). Playwright already seeds an admin session via `globalSetup` (F0.3) so the `seedLibrary` helper kept working without modification.

---

[← Back to roadmap summary](0-0-summary.md)
