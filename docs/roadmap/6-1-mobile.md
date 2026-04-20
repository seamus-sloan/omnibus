# F6.1 — Mobile app

**Phase 6 · Mobile** · **Priority:** P2

Dioxus Native feature parity for browse / read / listen on iOS + Android.

## Objective

Build out the existing `mobile/` crate into a full-featured native app: first-launch server-URL setup, authenticated browse / search, offline download, in-app reader, in-app audio player with background playback + system media controls, progress sync.

## User / business value

The feature that makes Omnibus an everyday tool instead of a desktop browser bookmark. Rust sharing between web and native means every backend feature lands on mobile without a platform-team bottleneck.

## Technical considerations

- **First-launch setup screen.** Today the server URL is hardcoded in [mobile/src/main.rs](../../mobile/src/main.rs); replace with a setup flow that persists into the app's secure-storage.
- **Offline download** writes the active format (EPUB or M4B) into app-private storage, indexed by book uuid.
- **Background audio** and system media controls are the Dioxus Native unknown — see open question below.
- **Progress sync** reuses [F2.1](2-1-progress-sync.md) directly via `reqwest` bearer-token auth from [F0.3](0-3-auth.md).
- Offline queue for progress writes buffered in a local SQLite; drained on reconnect.

## Dependencies

- [F2.1 Progress sync service](2-1-progress-sync.md).
- [F4.1 Native Kobo sync](4-1-kobo-sync.md) — protocol reused for offline queue semantics.

## Changes from v1

- v1 #14 "add `dioxus-mobile` crate target" is already done; that was the prototype. Scope shifts to feature parity and polish.

## Open question

- Does Dioxus Native give us enough access to `AVAudioSession` (iOS) / `MediaSession` (Android) for background audio + lock-screen controls, or do we need a platform-bridge crate? **Prototype required before committing scope.**

---

[← Back to roadmap summary](0-0-summary.md)
