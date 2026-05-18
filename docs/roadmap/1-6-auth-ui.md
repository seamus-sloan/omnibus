# F1.6 — Auth UI polish

**Phase 1 · Browse & discovery** · **Priority:** P2

Polish for the login and registration pages once the [F0.3](0-3-auth.md) backend is in place. Frontend-only.

## Objective

Replace the bare username/password forms in [frontend/src/pages/auth.rs](../../frontend/src/pages/auth.rs) with the auth shell and form primitives from the design canvas: a two-pane layout, semantic form fields with success/error/info states, a presentational password strength meter, a first-run setup wizard, and the failure-mode screens (server unreachable, validation errors). Several screens in the canvas depend on backend work [F0.3](0-3-auth.md) deliberately deferred to [F5.4](5-4-admin-panel.md) — those are captured here as **P3 / deferred** TODOs so the design intent is recorded without dragging deferred backend scope back into Phase 1.

## User / business value

The current pages render `error running server function: HTTP 401: unauthorized (details: None)` on the landing route for anonymous users and offer no visual treatment beyond browser defaults — the first impression of a self-hosted product where the user is the operator. Polish is a one-time investment that lifts the first impression and gives every later auth-touching screen (admin panel, settings → profile, device list) a shared component vocabulary.

## Technical considerations

- New shared primitives live under `frontend/src/components/auth/` so the deferred F5.4 screens (forgot/reset/lockout) can reuse them without re-litigating the visual language:
  - `AuthShell` — split-pane wrapper (left art panel with tilted spines + tagline, right form column).
  - `Field` — label + input + `hint` / `error` / `success` slots with the accent-ring visual.
  - `Banner` — `err` / `warn` / `info` / `ok` kinds with optional `action` slot.
  - `StrengthMeter` — four-segment presentational meter (no backend policy enforcement).
- Components must render identical markup under SSR and WASM hydration — no `cfg(feature = "web")` gates inside component bodies (the data-fetch and submit layers stay feature-gated, the rendered tree does not).
- Reuse the existing transport surface — `crate::data::login`, `crate::data::register`, `crate::data::current_user`, `crate::data::token_store::subscribe` — and the `data-testid` contract already exercised by [F0.3](0-3-auth.md)'s pending Playwright spec (`login-form`, `register-form`).
- Honor the design system tokens (`--accent`, `--ink-{0..3}`, `--bg-{0..3}`, `--line`, `--line-2`, `--ok`, `--warn`, `--bad`, `--serif`, `--mono`). The existing `auth-card` CSS namespace expands rather than gets replaced — incremental migration keeps screenshots reviewable per PR.
- Strength meter is **presentational**. Actual password policy stays on the server (argon2 hashing, length minimum). No client-side enforcement that the server doesn't also enforce.

## Dependencies

- [F0.3 Auth](0-3-auth.md) — backend, transport, extractors. All shipped.
- [F5.4 Admin panel](5-4-admin-panel.md) — SMTP, password-reset tokens, lockout policy, WebAuthn / passkey strategy. All deferred TODOs below block on this.

## Risks

- **Scope creep into deferred backend.** Each P3 TODO below has a tempting frontend-only stub that would land hollow without the backend. Hold the line: P3 items wait for F5.4.
- **Hydration drift.** Auth pages are the first thing an anonymous user hits, so any SSR/WASM mismatch is the most visible kind of regression. Per the project's prior hydration-mismatch incidents, no feature-gating inside component bodies.

## TODOs

### `AuthShell` split-pane layout primitive

**What:** Build the two-pane wrapper component matching the design canvas: a left art panel (brand mark, decorative tilted book spines, tagline copy block) and a right form column (kicker label, optional title + lede, slot for the form). Used by every auth screen below.

**Why:** Every auth screen in the design shares this shell. Building it once means the deferred F5.4 screens (forgot/reset/lockout) drop in without re-implementing the layout.

**Context:** `AuthShell` lives at `frontend/src/components/auth/shell.rs`. Props: `kicker: &str`, `title: Element`, `lede: Option<&str>`, `accent: Option<&str>`. Spines decorate-only — pull a stable subset of books from the existing book metadata so they don't churn between renders. Falls back gracefully when the library is empty (use a generic placeholder spine palette).

**Effort:** S
**Priority:** P1
**Depends on:** None.

### `Field` / `Banner` / `StrengthMeter` primitives

**What:** Three small components under `frontend/src/components/auth/`:
- `Field` — `label`, `hint`, `error`, `success`, `action` props. Renders an accent ring + tinted shadow per state. Owns the label/input pairing so consumers stop hand-writing `settings-field` div + label + input triplets.
- `Banner` — `kind: BannerKind` (`Err` / `Warn` / `Info` / `Ok`), `title`, `message`, `action`, `dismissable`. Maps to the design's icon + color treatment.
- `StrengthMeter` — `score: u8` (0..=4) + `label: &str`. Four-segment bar, color tier per score.

**Why:** Without shared primitives, every screen below re-implements the styling and we end up with three flavors of "error tinted input." Build the vocabulary once.

**Context:** All three are pure presentational components — no signals, no transport. CSS classes extend the existing `auth-*` namespace (`auth-msg-err`, `auth-banner-*`, etc.) already present in `omnibus.css`.

**Effort:** M
**Priority:** P1
**Depends on:** None.

### Polished login default screen

**What:** Rewrite `LoginPage` ([frontend/src/pages/auth.rs:29](../../frontend/src/pages/auth.rs)) to use `AuthShell` + `Field` + the existing `crate::data::login` transport. Add: "Keep me signed in for 30 days" checkbox bound to a session-TTL choice on the login request, server footer line (`omnibus.local · v0.4.1`), forgot-password link routing to the (stub) recovery page. Preserve the `data-testid="login-form"` contract.

**Why:** The current page is the production user's first impression and currently looks half-finished.

**Context:** The "keep me signed in" toggle requires `RemoteLoginRequest` to grow an optional `session_ttl` field. Default keeps current behavior; the long-TTL option opts into a 30-day session. Backend session expiry policy already exists in the sessions table — only the request-side wiring is new.

**Effort:** M
**Priority:** P1
**Depends on:** AuthShell + Field/Banner/StrengthMeter primitives above.

### Polished register default screen

**What:** Rewrite `RegisterPage` ([frontend/src/pages/auth.rs:111](../../frontend/src/pages/auth.rs)) to use `AuthShell` + `Field` + `StrengthMeter`. Add a presentational password-needs checklist (length, casing, number/symbol — three checks only; **no HIBP breach check**, see deferred TODO). Add the terms acknowledgment checkbox from the design ("I understand that the admin can see…"). Preserve the `data-testid="register-form"` contract.

**Why:** Matches the login screen's polish so the two pages feel coherent.

**Context:** Strength meter is purely presentational — server still enforces minimum length on `POST /api/auth/register`. The terms-ack checkbox does not gate submission yet; it's there to set the right expectations for self-hosted multi-user setups.

**Effort:** M
**Priority:** P1
**Depends on:** AuthShell + Field/Banner/StrengthMeter primitives above.

### Register validation errors UX

**What:** Surface per-field validation errors inline (existing-username, password-too-short) using `Field`'s `error` slot, plus a top-of-form `Banner` summarizing the count of fixable fields. Disable submit while errors are unresolved; the button label reads "Fix N fields to continue".

**Why:** The current page collapses every error into a single string at the bottom of the form. Inline errors with focused state are table-stakes for a polished register flow.

**Context:** Backend already returns structured errors via `crate::data::register` — wire them to the right Field. New error variants (e.g. "weak password") should ride the existing `RegisterError` enum rather than spawning a new transport shape.

**Effort:** M
**Priority:** P1
**Depends on:** Polished register default screen above.

### Server-unreachable login screen

**What:** When `POST /api/auth/login` fails with a network error (vs. a 4xx from the server), swap the error display for the design's full server-unreachable banner: title + message + Retry button + "open offline shelf" CTA (CTA stubbed until offline reading exists).

**Why:** On mobile, network errors are common (server paused, `adb reverse` not run, Tailscale dropped). The current "enter a username and password" error string is unhelpful and conflates network failure with user error.

**Context:** Distinguish transport errors from HTTP errors in `crate::data::mobile_login` / `crate::data::login` — bubble a typed variant rather than a flattened `String`. The `adb reverse` hint should only render on Android debug builds.

**Effort:** M
**Priority:** P2
**Depends on:** AuthShell + Banner primitives.

### First-run setup wizard

**What:** When the server has zero users and `registration_enabled` is open, replace the login page with a 3-step wizard: (1) server name, (2) owner account creation, (3) library paths. Steps 2 and 3 wire to existing endpoints (`POST /api/auth/register` and `POST /api/settings` respectively). Step 1 needs a new server-name setting key.

**Why:** Today the first user lands on a generic register page with no signal that they're creating the admin account, and library paths are configured later from a different screen. The wizard makes the "first-run, you are the owner" story explicit.

**Context:** Detect first-run via `GET /api/auth/me`'s response (or a new `GET /api/auth/setup-state` endpoint) — when `users_count == 0`, redirect from `/login` to `/setup`. The wizard reuses `Field` / `Banner` primitives. The stepper component itself is small and lives next to `AuthShell`. The "skip · use Unix passwords" link from the design is intentionally not implemented — out of scope for a SQLite-only deployment.

**Effort:** L
**Priority:** P2
**Depends on:** AuthShell + Field/Banner primitives. A new server-name settings key (small backend change).

### Deferred — Forgot-password page

**What:** Build the recovery request page (email-or-username field, "Send reset link" CTA, fallback banner explaining the admin can issue a one-time token when SMTP is not configured).

**Why:** Captured here so the link from the polished login screen has a target, even if that target stubs until F5.4 lands the backend.

**Context:** Depends on [F5.4](5-4-admin-panel.md) for the SMTP integration and the admin-issued reset-token endpoint. UI can be built ahead of the backend but should not ship to users until both halves exist.

**Effort:** S
**Priority:** P3 (deferred)
**Depends on:** F5.4 SMTP + reset-token endpoints.

### Deferred — Reset-password page

**What:** Build the confirm-new-password page reached from a reset link, with the "sign out all other devices" toggle from the design and a strength meter on the new password input.

**Why:** Completes the recovery flow opened by the forgot-password page above.

**Context:** Depends on [F5.4](5-4-admin-panel.md) reset-token verification endpoint and a session-revocation-by-user endpoint (the latter has no equivalent today — only single-session logout exists).

**Effort:** S
**Priority:** P3 (deferred)
**Depends on:** F5.4 reset-token + bulk session revoke endpoints.

### Deferred — Account-locked screen

**What:** Build the cooling-off screen with a countdown timer and an "ask admin to unlock" CTA when the server rejects login with a lockout status.

**Why:** F0.3 keeps rate-limit only for v1.0 (10 req / 60s per IP). True per-account lockout, with admin-side unlock, is part of [F5.4](5-4-admin-panel.md).

**Context:** Depends on F5.4 adding a per-account lockout policy (failed-attempts counter, unlock timestamp) and an admin endpoint to clear it.

**Effort:** S
**Priority:** P3 (deferred)
**Depends on:** F5.4 lockout policy + admin unlock endpoint.

### Deferred — Wrong-password with attempts-remaining counter

**What:** When login fails with the wrong password and a lockout policy is active, render the "Two more attempts before this account is locked for 15 minutes" banner from the design.

**Why:** Only meaningful once the lockout policy above exists; without it the counter is misleading.

**Context:** Depends on the account-locked screen TODO and the F5.4 lockout policy that underpins it.

**Effort:** S
**Priority:** P3 (deferred)
**Depends on:** Account-locked screen TODO above (transitively, F5.4).

### Deferred — Passkey / WebAuthn login

**What:** Add the passkey button below the password form on login (and as an alternative on the wrong-password screen).

**Why:** [F0.3](0-3-auth.md) shaped the `AuthStrategy` trait to accept a WebAuthn implementation as a second strategy without schema rework. Building the UI now would leave the button dead until that strategy exists.

**Context:** Depends on a `WebAuthnStrategy` implementation (likely under F5.4 admin → security) and the corresponding server endpoints for credential creation / assertion.

**Effort:** M
**Priority:** P3 (deferred)
**Depends on:** WebAuthn `AuthStrategy` implementation.

### Deferred — HIBP k-anonymity password breach check

**What:** Add the "Not in any known breach" requirement to the register password-needs checklist, backed by a k-anonymity query against the [Have I Been Pwned passwords API](https://haveibeenpwned.com/API/v3#PwnedPasswords).

**Why:** F0.3 keeps password policy minimal (length only) for v1.0. Breach checks add a useful signal but require outbound HTTP, which Omnibus otherwise avoids by design.

**Context:** Depends on an outbound-HTTP allowlist policy and an admin toggle (some self-hosters will want to disable it on principle). Lives under [F5.4](5-4-admin-panel.md) admin settings.

**Effort:** M
**Priority:** P3 (deferred)
**Depends on:** F5.4 outbound-HTTP policy + admin toggle.

## Status

Brand new — not started. Backend already in place via [F0.3](0-3-auth.md); this initiative is frontend-only on top of it.

---

[← Back to roadmap summary](0-0-summary.md)
