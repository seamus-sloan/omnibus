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

---

[← Back to roadmap summary](0-0-summary.md)
