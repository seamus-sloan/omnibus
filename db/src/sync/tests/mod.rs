//! Unit tests for the full-library sync entry points — `replace_books`,
//! `sync_books`, `sync_audiobooks` and their progress-reporting variants —
//! split by sub-topic into the sibling modules below. Each drives the composed
//! write path end-to-end against an in-memory DB; the per-helper contracts
//! live in `books/tests` and `audiobooks/tests`.

mod audiobooks;
mod buckets;
mod identifiers;
mod progress;
mod removed;
mod replace;
