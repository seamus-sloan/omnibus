# F0.3 — Auth

**Phase 0 · Foundations** · **Priority:** P0

Multi-user authentication with sessions; first-user-admin. Moved up from v1 #7.

## Objective

Deliver registration, login, logout, session persistence, and route guards before any feature that touches per-user state (progress, ratings, libraries, Kindle address). First registered user is promoted to admin automatically; an `OMNIBUS_INITIAL_ADMIN` env-var provides an ops-recovery escape hatch.

## User / business value

Every feature touching user state needs it. Building those first and retrofitting auth is strictly more work than doing auth early. v1's ordering (auth at #7, personalization features before it) implies throwaway single-user prototypes — we skip that.

## Technical considerations

- `argon2` crate for password hashing.
- `tower-sessions` backed by SQLite for web session cookies.
- Explicit permission **columns** (`is_admin BOOL`, `can_upload BOOL`, `can_edit BOOL`, `can_download BOOL`) rather than a role bitmask — bitmasks are opaque and migration-hostile (see [calibre-inspection recommendation #9](../calibre-inspection/7-recommendations.md)).
- `auth_required` / `admin_required` axum extractors, mirroring the shape of `StateExtractor`.
- Unified auth model across web (cookies) and mobile (bearer tokens) — both flow into the same session table.
- **OIDC/SSO as a day-one extension point, not a v2 bolt-on.** Shape the auth layer so a second `AuthStrategy` (OIDC via the `openidconnect` crate, full PKCE, group→role mapping) slots in without schema rework. ABS's [OidcAuthStrategy.js](https://github.com/advplyr/audiobookshelf/blob/master/server/auth/OidcAuthStrategy.js) is ~560 lines because it was bolted on — designing the trait up front avoids that cost. See [ABS recommendation #11](../audiobookshelf-inspection/7-recommendations.md).
- **Device rows** for registered ereaders / sync destinations (Kobo, Kindle, OPDS clients) — *not* for browser sessions. The [`devices` table](../../db/migrations/0004_auth.sql) carries `name`, `client_kind`, `client_version`, `created_at`, `last_seen_at`, `last_seen_ip` so an admin can see which physical ereader is registered against an account and revoke a specific one. Browser/mobile login state belongs in the `sessions` table, not here. The current login handler ([server/src/auth/handlers.rs:183](../../server/src/auth/handlers.rs)) opportunistically registers a device row when a `device_name` is supplied — fine for mobile, but the canonical writers will be the Kobo registration handshake ([F4.1](4-1-kobo-sync.md)), the Kindle delivery flow ([F4.3](4-3-kindle.md)), and the OPDS client registration ([F4.2](4-2-opds.md)). ABS pattern: [Device.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Device.js).
- **Per-route authorization is its own initiative.** The permission columns above only matter if handlers consult them. Wiring `AuthUser` / `AdminUser` onto every protected handler is scoped under [F0.7 Per-route authorization](0-7-route-authorization.md); F0.3 is responsible for the columns and the extractors, F0.7 is responsible for using them.

## Dependencies

- [F0.1](0-1-schema-refactor.md) — `users` table plus FK'd relationships to progress, ratings, etc.

## Risks

- Session semantics in Dioxus fullstack + mobile need care — cookies on web, bearer tokens on mobile. Prototype both before committing (see [next steps](0-0-summary.md#8-immediate-next-steps)).

## Cut from v1

The "first registered user is admin" rule stays, plus the `OMNIBUS_INITIAL_ADMIN` env-var for recovery. No email verification, no password reset flow in v1.0 — both land with [F5.4 admin panel](5-4-admin-panel.md).

## TODOs

### Web `/login` route guard at `ScreenLayout` level

**What:** When an anonymous user hits `/`, `/settings`, or `/books/:id` on the web client, redirect them to `/login` after hydration the way mobile already does in `ScreenLayout` ([frontend/src/lib.rs:101](../../frontend/src/lib.rs)).

**Why:** Today the web SSR shell renders for anyone; only the `/api/*` calls 401 silently. Worse than just a blank shell — the failed server-function call surfaces the literal string `error running server function: HTTP 401: unauthorized (details: None)` in the page, which looks broken to the user and leaks transport-level error text. The 2026-04-26 QA pass ([qa-report](../qa/qa-report-2026-04-26.md)) flagged this as the most visible "looks broken" follow-up after the origin_check fix; the audit re-pass on the same day confirmed the leaked-error string on `/` and `/settings`.

**Context:** Mirror the mobile pattern. The web `ScreenLayout` pings `/api/auth/me` (already exposed via `data::current_user()`) once on hydrate, drives a `use_signal(authed)`, and `nav.replace(Route::Login {})` if 401. SSR keeps rendering the empty shell — the redirect happens client-side after one RTT, so SSR markup stays deterministic and hydration doesn't break. Auth-shell screens (`Login`/`Register`) already bypass `ScreenLayout` so they stay reachable to anonymous users.

**Effort:** S
**Priority:** P1
**Depends on:** None.

### Playwright auth E2E flow

**What:** New `ui_tests/playwright/tests/flows/auth.spec.ts` covering register → authenticated `/api/*` call → click **Log out** → land on `/login` → log back in → assert landing.

**Why:** Auth has no E2E coverage today. The 2026-04-26 origin_check regression (cookie-authed POSTs 403'ing through the dx-fullstack proxy) had passing unit tests but no E2E that would have caught it. Codifying the happy path as a Playwright spec means the next break in this surface trips CI instead of landing in main.

**Context:** Stable testids are already in the markup: `data-testid="login-form"` and `data-testid="register-form"` on the auth pages, `data-testid="logout-button"` on the new TopNav slot. Use the existing `expectMutation` helper for `POST /api/auth/register`, `/api/auth/logout`, `/api/auth/login` round-trips and `expectNavVisible` for the post-login layout. Once the web route-guard TODO above lands, extend this spec with an anonymous `/ → /login` redirect assertion.

**Effort:** S
**Priority:** P2
**Depends on:** None (independent of the route-guard TODO; either can land first).

### Update `devices.last_seen_at` on session use

**What:** When a request lands with a session/bearer that resolves to a device row, bump that row's `last_seen_at` (and, when the request originates from an ereader/Kobo/Kindle endpoint, write a truncated `last_seen_ip` per the `/24` and `/48` rule from the schema comment in [db/migrations/0004_auth.sql:34](../../db/migrations/0004_auth.sql)). Today the column has a default but no `UPDATE` site, so the value is frozen at registration time.

**Why:** "Last seen" is the audit signal that lets an admin tell a stale Kobo registration apart from one that's actively syncing — without it the admin panel's device list (planned for [F5.4](5-4-admin-panel.md)) is decorative. Rate-limit the touch the same way `sessions.last_used_at` is touched (don't write on every request) so an actively-syncing client doesn't hammer the row.

**Effort:** S
**Priority:** P2
**Depends on:** None. The column already exists; this is a writer + a touch threshold.

## Status

Auth core landed across multiple PRs: registration, login, logout endpoints, session persistence on web cookies + mobile bearer tokens (PR4), first-user-admin promotion, `OMNIBUS_INITIAL_ADMIN` recovery hook, CSRF `origin_check` middleware (10 req / 60 s per-IP) on `/api/auth/{login,register}`, the `AuthStrategy` trait shaped for OIDC, server-side session revocation on logout, and a DB-backed `registration_enabled` toggle that closes after the first user (boot hook reopens it for the named `OMNIBUS_INITIAL_ADMIN`). The `AuthUser` and `AdminUser` extractors and the `is_admin` / `can_upload` / `can_edit` / `can_download` columns are in place; **wiring them onto every protected handler is tracked separately under [F0.7 Per-route authorization](0-7-route-authorization.md)** so this initiative can close once the TODOs below land.

The 2026-04-26 QA pass ([qa-report](../qa/qa-report-2026-04-26.md), [PR #43](https://github.com/seamus-sloan/omnibus/pull/43)) cleared the cookie-authed-POST regression behind the dx-fullstack proxy and added the missing logout button to TopNav. The same-day audit re-pass spun out F0.7 (per-route authorization), [F5.4](5-4-admin-panel.md) admin-side TODOs for the `registration_enabled` toggle and the device/session list & revoke endpoints, and the `last_seen_at` follow-up captured above.

Out of scope for v1.0 (deliberately): email verification, password reset, account lockout (rate limit only), audit log, session sliding-window expiry. All land later — most under [F5.4](5-4-admin-panel.md).

---

[← Back to roadmap summary](0-0-summary.md)
