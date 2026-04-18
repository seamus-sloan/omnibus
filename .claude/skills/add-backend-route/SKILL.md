---
name: add-backend-route
description: Recipe for adding a new Axum route to the omnibus server — page, API endpoint, or full-stack feature. Triggers when the user asks to add a new page, endpoint, handler, or server feature.
---

# Add a backend route

Follow these steps in order. Every step has a rule that governs it — consult it if you're unsure.

## 1. Decide the route shape

- **Page (HTML):** extend the `Route` enum in [server/src/frontend/mod.rs](../../../server/src/frontend/mod.rs) and add a page component under `server/src/frontend/pages/`. Handler returns `Html(...)` after SSR.
- **JSON API:** no `Route` enum change. Handler returns `Json(...)` directly. Path convention: `/api/<resource>`.

## 2. Add the handler

In [server/src/backend.rs](../../../server/src/backend.rs):

- Register the route on the `Router` with `.route(...)`.
- Use `State<AppState>` for DB access.
- Propagate errors with `anyhow::Error` (see [02-error-handling.md](../../rules/02-error-handling.md)).

## 3. Add the DB query (if needed)

In [server/src/db.rs](../../../server/src/db.rs):

- Define a typed `DbError` variant for any new failure mode.
- Use `sqlx::query_as!` or `sqlx::query!` for compile-time checking.

## 4. Add tests

Required per [03-unit-testing.md](../../rules/03-unit-testing.md):

- **DB:** happy path + not-found + constraint violation.
- **Handler:** 200 + 4xx + 5xx (DB failure). Use `tower::ServiceExt::oneshot` against an in-memory DB.
- **Frontend component (if added):** rendered output contains expected content.

## 5. Add Playwright coverage (user-facing changes only)

See [add-playwright-flow](../add-playwright-flow/SKILL.md). Add a new spec under `ui_tests/playwright/tests/flows/` with one layout test + action tests (happy + error paths).

## 6. End-of-session

Run [99-end-of-session.md](../../rules/99-end-of-session.md). Update [CLAUDE.md](../../../CLAUDE.md) module-structure section if a new file was introduced.
