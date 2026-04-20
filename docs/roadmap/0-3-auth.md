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

---

[← Back to roadmap summary](0-0-summary.md)
