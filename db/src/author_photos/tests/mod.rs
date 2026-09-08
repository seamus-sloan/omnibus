//! Tests for `db::author_photos`, split by sub-topic into the sibling
//! modules below; the author-seeded pool and Open Library config fixtures
//! they share live here. Covers the cascade resolver, the SSRF guard, the
//! remote-image fetch, and the provider-cover allowlist gates.

mod allowlist;
mod cascade;
mod remote_fetch;
mod ssrf_guard;
