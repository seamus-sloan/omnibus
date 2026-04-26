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
- **Device rows** for registered clients (`device_id`, `client_name`, `client_version`, `last_seen`, `ip_address`) so admin can see who's connected and revoke a specific phone. ABS pattern: [Device.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/Device.js).

## Dependencies

- [F0.1](0-1-schema-refactor.md) — `users` table plus FK'd relationships to progress, ratings, etc.

## Risks

- Session semantics in Dioxus fullstack + mobile need care — cookies on web, bearer tokens on mobile. Prototype both before committing (see [next steps](0-0-summary.md#8-immediate-next-steps)).

## Cut from v1

The "first registered user is admin" rule stays, plus the `OMNIBUS_INITIAL_ADMIN` env-var for recovery. No email verification, no password reset flow in v1.0 — both land with [F5.4 admin panel](5-4-admin-panel.md).

## TODOs

### Web `/login` route guard at `ScreenLayout` level

**What:** When an anonymous user hits `/`, `/settings`, or `/books/:id` on the web client, redirect them to `/login` after hydration the way mobile already does in `ScreenLayout` ([frontend/src/lib.rs:101](../../frontend/src/lib.rs)).

**Why:** Today the web SSR shell renders for anyone; only the `/api/*` calls 401 silently. Users see "No ebooks found" or a blank settings form with no cue that they're unauthenticated. The 2026-04-26 QA pass ([qa-report](../qa/qa-report-2026-04-26.md)) flagged this as the most visible "looks broken" follow-up after the origin_check fix.

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

## Status

In progress. Auth core landed across multiple PRs: registration, login, logout endpoints, session persistence on web cookies + mobile bearer tokens (PR4), first-user-admin promotion, `OMNIBUS_INITIAL_ADMIN` recovery hook, CSRF `origin_check` middleware, and per-IP rate limiting on `/api/auth/{login,register}`. The 2026-04-26 QA pass ([qa-report](../qa/qa-report-2026-04-26.md), [PR #43](https://github.com/seamus-sloan/omnibus/pull/43)) cleared the cookie-authed-POST regression behind the dx-fullstack proxy and added the missing logout button to TopNav. Outstanding work captured in the TODOs above.

---

[← Back to roadmap summary](0-0-summary.md)
