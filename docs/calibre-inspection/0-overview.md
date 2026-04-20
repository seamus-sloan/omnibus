# Calibre-Web deep-dive for Omnibus

A source-level study of [janeczku/calibre-web](https://github.com/janeczku/calibre-web) (Flask + SQLAlchemy, Python 3) intended to guide Omnibus' Rust/Dioxus reimplementation. The goal is to reuse everything Calibre-Web got right, avoid its known pain points, and exploit the parts of our stack (axum + sqlx + Dioxus fullstack) that Flask + SQLAlchemy cannot match.

This analysis is split across several documents for navigability. Start here; jump to sections as needed. The [roadmap](../roadmap/0-0-summary.md) consumes these findings and maps them onto concrete initiatives.

## Contents

1. [Where Omnibus stands today](1-omnibus-state.md)
2. [Calibre-Web feature inventory](2-feature-inventory.md)
3. [Performance pain points in Calibre-Web](3-performance-pain-points.md)
4. [Where Dioxus / Rust wins](4-dioxus-rust-wins.md)
5. [Schema details worth copying (and improving)](5-schema-details.md)
6. [API surface](6-api-surface.md)
7. [Recommendations for Omnibus](7-recommendations.md)

## Unconfirmed claims

Items that needed further source-reading out of scope for this pass:

- The exact byte layout of Calibre-desktop's `books_fts` virtual table.
- Whether PR [#3476](https://github.com/janeczku/calibre-web/pull/3476)'s `selectinload` landed on stable releases or only `master`.
- The precise path the `FileSystem` cache helper resolves to at runtime.

---

[← Back to roadmap summary](../roadmap/0-0-summary.md)
