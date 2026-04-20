# AudioBookShelf deep-dive for Omnibus

A source-level study of [advplyr/audiobookshelf](https://github.com/advplyr/audiobookshelf) v2.33.2 (Node.js + Express + Sequelize + Nuxt 2) intended to guide Omnibus' Rust/Dioxus reimplementation on the audiobook/podcast side. Omnibus' existing EPUB-oriented foundation is not in conflict with ABS's approach — ABS is strongest exactly where Omnibus is weakest (audio chunking, HLS, podcast RSS, playback sessions, cross-device progress sync) and makes several architectural choices we should *not* copy.

This analysis is split across several documents for navigability. Start here; jump to sections as needed. The [roadmap](../roadmap/0-0-summary.md) consumes these findings and maps them onto concrete initiatives.

## Contents

1. [Where Omnibus stands today](1-omnibus-state.md)
2. [AudioBookShelf feature inventory](2-feature-inventory.md)
3. [Performance pain points in AudioBookShelf](3-performance-pain-points.md)
4. [Where Dioxus / Rust wins](4-dioxus-rust-wins.md)
5. [Schema details worth copying (and improving)](5-schema-details.md)
6. [API surface](6-api-surface.md)
7. [Recommendations for Omnibus](7-recommendations.md)

## Unconfirmed claims

Items that needed further source-reading out of scope for this pass:

- Whether the `nusqlite3` extension path (see `Database.loadExtension`) is actually shipped on the default Docker image, or a power-user opt-in.
- The precise retry/back-off characteristics of the single-slot `PodcastManager.currentDownload` pipeline on flaky feeds.
- Whether the experimental Next.js client under `REACT_CLIENT_PATH` is production-ready or still a spike.

---

[← Back to roadmap summary](../roadmap/0-0-summary.md)
