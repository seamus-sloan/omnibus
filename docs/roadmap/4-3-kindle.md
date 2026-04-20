# F4.3 — Kindle delivery

**Phase 4 · Device sync** · **Priority:** P2

Email-to-Kindle using EPUB directly, no conversion.

## Objective

User configures `kindle_email` in their profile; admin configures SMTP; book detail page surfaces a "Send to Kindle" action that emails the EPUB as an attachment via the [F0.5 worker](0-5-background-worker.md).

## User / business value

Kindle is the dominant e-reader; the send-to-email flow is how most users load books. No conversion step means no `ebook-convert` dependency in the hot path.

## Technical considerations

- **EPUB direct — no conversion.** Amazon has accepted `.epub` via send-to-Kindle email since 2024 ([assumption A4](0-0-summary.md#assumptions)). The v1 implication that we'd need MOBI/AZW3 conversion is outdated.
- SMTP config lives in admin settings ([F5.4](5-4-admin-panel.md)). Single SMTP config shared with registration/password-reset email once those ship.
- Send job runs on the [F0.5 worker](0-5-background-worker.md); failures log to [F5.2 observability](5-2-observability.md).

## Dependencies

- [F0.3 Auth](0-3-auth.md).
- [F0.5 Background worker](0-5-background-worker.md).

## Changes from v1

- **No conversion step.** v1 implied MOBI/AZW3; epub direct is current-best-practice.
- **Auto-sync-on-new-book deferred.** The per-library deliverable rule engine is complex; ship manual "Send" first. Revisit post-v1.0.

---

[← Back to roadmap summary](0-0-summary.md)
