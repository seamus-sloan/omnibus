# F5.4 — Admin panel

**Phase 5 · Admin & hygiene** · **Priority:** P2

Unified admin surface absorbing metadata edit, observability, SMTP, scan paths, and user management.

## Objective

Single admin route tree with sub-sections for: users (create/edit/permissions), scan paths (library roots + kinds), SMTP config, metadata edit ([F5.1](5-1-metadata-edit.md)), log viewer + background-task dashboard ([F5.2](5-2-observability.md)), kepubify / `ebook-convert` path config.

## User / business value

v1 #13. The operator surface for the product — the place a self-hoster goes to keep the server healthy.

## Technical considerations

- One SMTP config shared across send-to-Kindle ([F4.3](4-3-kindle.md)), registration email, and password reset. Don't duplicate settings.
- Scan path management evolves the current `Settings` shape: today it's two scalar paths; target is `Vec<LibraryPath { path, kind }>` (see [summary §7 open question 2](0-0-summary.md#open-questions)).
- Permission editing exposes the explicit boolean columns from [F0.3](0-3-auth.md) — never a bitmask ([recommendation #9](../calibre-inspection/7-recommendations.md)).

## Dependencies

- [F5.1 Metadata edit](5-1-metadata-edit.md).
- [F5.2 Observability](5-2-observability.md).
- [F4.3 Kindle delivery](4-3-kindle.md) (SMTP config sharing).

## Changes from v1

- Absorbs F5.1 and F5.2 as sub-sections rather than freestanding features.
- SMTP consolidated into one place.

## TODOs

### Admin endpoint to toggle `registration_enabled`

**What:** Expose `POST /api/admin/registration` (`AdminUser`-gated) that flips the `registration_enabled` setting in the [`settings` table](../../db/migrations/0004_auth.sql) via [`db::auth::set_registration_enabled`](../../db/src/auth.rs). Surface it in the admin panel as a toggle on the user-management sub-section.

**Why:** [F0.3 auth](0-3-auth.md) closes registration after the first user lands. Today the only ways to reopen the gate are direct SQL or the `OMNIBUS_INITIAL_ADMIN` boot hook (which only promotes an *existing* user). Until this endpoint exists, an admin literally cannot onboard user #2 through the running app — captured during the [2026-04-26 audit](../qa/qa-report-2026-04-26.md). Pair the toggle with a "create user" admin form so the admin can either (a) flip the gate and let users self-register, or (b) admin-create a user account directly.

**Effort:** S (toggle endpoint + tests). M if paired with the admin user-creation form.
**Priority:** P2
**Depends on:** [F0.7 Per-route authorization](0-7-route-authorization.md) for the `AdminUser` enforcement to be real.

### Device & session list / revoke endpoints

**What:** Two admin-gated endpoints plus matching self-service equivalents:

- `GET /api/admin/users/{id}/devices` and `GET /api/admin/users/{id}/sessions` for the admin panel.
- `DELETE /api/admin/sessions/{id}` (admin-revoke a session) and `DELETE /api/admin/devices/{id}` (admin-revoke an ereader registration).
- `GET /api/auth/sessions` and `DELETE /api/auth/sessions/{id}` for self-service: a logged-in user can list their own sessions and revoke any except the current one. Same shape for `/api/auth/devices` once Kobo / Kindle write to the `devices` table per [F0.3](0-3-auth.md).

The DB-layer revokers ([`revoke_session`](../../db/src/auth.rs), [`revoke_all_sessions_for_user`](../../db/src/auth.rs), [`list_devices_for_user`](../../db/src/auth.rs)) already exist; the work is HTTP wiring, request authorization (admin-gated vs self-only), and the admin-panel UI.

**Why:** A user with a lost phone has no path to log it out today; an admin investigating a compromise has no surface to see or kill a specific session. Ship the self-service variant in the same change so users don't have to file a support request to revoke their own credentials.

**Effort:** M
**Priority:** P2
**Depends on:** [F0.7 Per-route authorization](0-7-route-authorization.md) for the role enforcement; partially parallelizable with the registration-toggle TODO above (shares an admin-panel sub-section).

---

[← Back to roadmap summary](0-0-summary.md)
