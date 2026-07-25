# Rust style guide

Long-form companion to [.claude/rules/05-rust-style.md](../.claude/rules/05-rust-style.md).
The rule file is the **normative** version — terse, scannable, no
rationale. This file is for the *why*: each rule, the reasoning, and
before/after examples drawn from real files in this repo.

If the rule file and this file disagree, the rule file wins.

---

## Comments & docs

### Module-level `//!`

**Rule:** every file gets a `//!` block of about 3–5 lines: what the
module is for, who calls it.

**Why:** module docs answer the question "do I want to be reading this
file at all?". A reader scanning crate src/ should be able to skim the
first paragraph of each file and know which one owns the behaviour they
care about. Anything longer (multi-section invariants, roadmap context)
belongs in commit messages and PR bodies — those have an audit trail, a
review pass, and don't rot when the issue numbers stop being
meaningful.

**Anti-pattern (since fixed).** `db/src/discovery.rs` once opened with a
17-line `//!` containing a `# Multi-tenancy invariant` H2 heading and
forward references to "Phase 4", issue #232, and an "F4.x" milestone —
the kind of context that rots the moment F4.x ships or #232 closes. It
has since been split into `discovery/{mod,authors,series,tags}.rs` with
short module docs; multi-tenancy context like that belongs in the PR
that introduces it, not the file.

**Pattern.** Something like:

```rust
//! F1.8 discovery-detail reads: one author or series with their books,
//! plus the global tag cloud. Returns DB-wide results; per-user ACL
//! scoping is not implemented.
```

What the module does, one notable invariant (DB-wide reads), end. No
roadmap, no issues, no H2 sections.

### Function `///`

**Rule:** every `pub` item gets a one-line `///` summary. Longer
rustdoc only when behaviour is surprising.

**Why:** when reading a call site, IDE hover or `cargo doc` should show
*what the function does* without making you open the source. One line
of useful summary beats five lines of obligatory `# Errors` /
`# Examples` boilerplate. Reserve longer docs for actual contracts: a
lock that callers must not hold across `.await`, a function that
mutates after returning `Err`, a retry budget.

**Anti-pattern (since fixed).** `get_author` in the old
`db/src/discovery.rs` once carried a 22-line rustdoc with three H2
sections (`# Bounded reads (issue #150)`, `# Multi-tenancy`) describing
what the cap used to be, what `book_count` means, and what F4.x would
require. The function is `async fn get_author(pool, author_id) ->
Result<Option<AuthorDetail>, sqlx::Error>`. Everything beyond "fetch an
author by ID with their books; `None` if missing" belonged in a const
doc or a PR description. It now lives in `db/src/discovery/authors.rs`
with a one-line summary.

**Pattern.** `list_files` in [db/src/scanner.rs](../db/src/scanner.rs):

```rust
/// Recursively walk `path` and return total file count plus
/// per-extension counts for each extension in `extensions` (compared
/// case-insensitively, without leading dot — e.g. `&["epub", "pdf"]`).
pub fn list_files(path: Option<&str>, extensions: &[&str]) -> LibrarySection {
```

Three lines. Tells you what it does and what the inputs look like.
Done.

### Inline `//` comments

**Rule:** explain *why*, never *what*. If you're narrating the
algorithm, the function probably needs to be split.

**Why:** the code is the *what*. Reading `// load overrides for the
visible books` above `let overrides = load_overrides_bulk(&pool,
&visible).await?;` is noise — the function name already says that.
Useful comments encode information the reader *can't* derive from the
code: a known SQLite quirk, a backwards-compat workaround, a
non-obvious invariant.

**Anti-pattern (since fixed).** `db/src/sync.rs` once had section markers
like `// --- Removed -------------------` and `// --- Backfill
---------------` scattered through a ~270-line `sync_books`. They were
nav aids for a function that shouldn't have been that long in the first
place. `sync.rs` is now a 46-line module file and `sync_books` lives in
`db/src/sync/books.rs`, split into per-bucket helpers (see
[Function shape](#function-shape)).

**Pattern.** A comment that says something the code doesn't:

```rust
// SQLite returns BLOB columns as Vec<u8>, but the migration that
// added this column predates the BLOB affinity rule, so older rows
// can come back as TEXT. Read both cases before treating it as
// corrupted data.
```

That's a `// why` comment worth keeping.

---

## Function shape

**Rule:** soft cap ~80 lines per function. Extract named helpers when a
function exceeds it, when it has clear staged sub-steps, or when a
stage is independently testable.

**Why:** a function you can read top-to-bottom on one screen is one you
can hold in your head while modifying. Long functions with section
comments lie about their structure — the comments suggest discrete
stages but the locals leak between them, so refactoring one stage
means re-reading the whole body. Named helpers make the stages
real: each has a signature, a test, and a name that documents intent.

**Model.** [db/src/worker/](../db/src/worker/) (split into
`{types,queue,exec,handlers,progress}.rs`). Each `impl` method does one
thing and most are short — `resource_key`, `kind`, `post`,
`await_completion` are all single-purpose.

**Anti-pattern (since fixed).** `db/src/sync.rs`'s ~270-line `sync_books`
did upsert + remove + change + new + backfill + FTS-rebuild in one body,
marked off with `// --- Removed`, `// --- Changed`, `// --- New`,
`// --- Backfill` dividers. It has since been extracted into
`sync_removed` / `sync_changed` / `sync_new` / backfill helpers under
[db/src/sync/](../db/src/sync/), with the top-level function now a thin
transaction wrapper that calls them in order — exactly the shape this
rule prescribes.

---

## File shape

**Rule:** soft cap ~800 lines per file. When crossed, split by
sub-topic using the `books/` subdirectory pattern — e.g.
`sync.rs` → `sync/{mod,books,authors,backfill}.rs`.

**Why:** files this big have stopped being single-responsibility — they
just collect "things that touch sync". Splitting them forces a naming
decision (what *is* the sub-topic?) which usually surfaces a
missing module boundary. The `books/` subdirectory pattern in this
repo already proves the shape works.

**Proven by this repo.** The `books/`, `sync/`, `discovery/`, `palette/`,
`metadata_overrides/`, and `worker/` subdirectory splits all started as
single 800–1700-line files and were broken up by sub-topic using exactly
this pattern. When a file crosses the cap, follow them rather than letting
it keep growing.

---

## Errors

**Rule:**
- Predictable failure space → `thiserror` enum in the module.
- Unpredictable failure space → `anyhow` with a contextual message.
- Coarse variants — group by failure mode, let the `#[error]` message
  carry the detail.
- Never return raw `sqlx::Error` across a module boundary.
- Handlers stay on `anyhow::Error` at the signature.

**Why predictable vs unpredictable:** when a caller can enumerate the
ways the call might fail, the typed enum carries useful information
through the `?` operator and to the UI. When the underlying system can
fail in arbitrary ways (filesystem walks, EPUB parsing, network
fetches), an exhaustive enum is a lie that future-you has to keep
updating — `anyhow` with a `with_context(...)` message is honest.

**Pattern (predictable).** [db/src/auth.rs](../db/src/auth.rs)
correctly chooses `thiserror`: login, registration, and session
validation each have a finite set of outcomes a UI renders
differently. `AuthError` was once over-granular but has since been
coarsened to the shape below — see "Coarse variants".

**Pattern (unpredictable).** `reindex` in
[db/src/indexer.rs](../db/src/indexer.rs):

```rust
pub async fn reindex(pool: &SqlitePool, library_path: &str) -> anyhow::Result<ReindexStats> {
    // ...
    if let Some(msg) = stat.error {
        anyhow::bail!("scan of {library_path} failed: {msg}");
    }
```

Good fit: filesystem walks can fail in many ways and the caller just
propagates. The format string carries the detail.

### Coarse variants

**Rule:** group by failure mode. One `PasswordInvalid(String)` beats
`PasswordTooShort { min }` / `PasswordTooLong { max }` /
`PasswordCommon` unless the caller actually branches on them.

**Anti-pattern (since fixed).** `db/src/auth.rs`'s `AuthError` once had
14 variants — three password rules, four username rules — even though
the login UI renders the same "invalid username or password" for every
credential failure and the registration form shows the `#[error]` text
directly. It has since collapsed to ~8 coarse variants
(`InvalidCredentials`, `Validation(String)`, `UsernameTaken`,
`SessionNotFound`, `AccountLocked`, `RegistrationDisabled`, transparent
`Db`, …), with the message doing the work — matching the Pattern below.

**Pattern.** Three to five variants, each genuinely different in how
the caller handles them:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("account is temporarily locked")]
    AccountLocked { until_unix: i64 },
    #[error("{0}")]
    Validation(String),          // password/username rules, etc.
    #[error("session not found or expired")]
    SessionNotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
```

### Never leak `sqlx::Error`

**Anti-pattern (since fixed).** `sync_books` once returned
`Result<(), sqlx::Error>` from a `pub` function, leaking the DB crate
into every downstream caller: any future migration to a different DB
crate becomes a workspace-wide rename, and `sqlx::Error` carries
unrelated variants (`PoolTimedOut`, `Configuration`, …) the caller
can't meaningfully handle. It now returns `anyhow::Result<()>` — the
internal `SyncError` enum in `db/src/sync/books.rs` is `pub(crate)`,
so external callers see only an opaque `anyhow::Error` and `sqlx::Error`
stays inside the `db` crate. Wrap, don't leak.

---

## Tests

### Placement

**Rule:** sibling file `<mod>/tests.rs` for anything non-trivial.
Inline `#[cfg(test)] mod tests` only for 1–2 trivial tests.

**Why:** prod files stay readable when the test block isn't padding
them. A `books/tests.rs` sibling is one file to open when you want
the tests, and one file to skip when you want the prod code. The
existing [db/src/books/tests.rs](../db/src/books/tests.rs) is the
model.

**Migration note.** Existing inline `mod tests` blocks are not in
violation by themselves; the rule kicks in when a module is being
touched and its inline tests are large enough that moving them out
makes the prod file more readable.

### Shared helpers

**Rule:** every crate has a `test_support` module
(`<crate>/src/test_support.rs`, gated
`#[cfg(any(test, feature = "test-support"))]`).

**Why:** every db test today recreates the in-memory pool and reseeds
fixtures with locally-defined helpers. Pulling them into one
`test_support` per crate gives the tests one canonical
`new_in_memory_pool()`, one `seed_minimal_books()`, one
`make_test_dir()`. New tests stop reinventing fixtures and start
agreeing on what "a seeded library" looks like.

The `feature = "test-support"` gate lets sibling crates depend on the
helpers in their *own* tests without exporting them in a release
build.

### Coverage

**Rule:** every `pub` fn gets one happy-path test plus one test per
`thiserror` variant it can return. Skip edge / boundary tests unless
the function does tricky math, parsing, or arithmetic. For
`anyhow`-returning functions, cover happy + one representative
failure.

**Why:** "exhaustive matrix" sounds rigorous but produces test files
where 80% of cases test the language, not the function. The variant
rule scales coverage to the *modeled* failure space — if the error
space is too big to test, the error enum is probably too granular
(see Coarse variants).

**Anti-pattern.** [db/src/scanner.rs](../db/src/scanner.rs) has six
happy-path tests and zero error tests, even though `list_files`
swallows several distinct error conditions silently (returns empty
sections on missing path, on I/O failures, etc.). Each silent
fallback should be a test asserting the contract.

### Naming

**Rule:** long sentence style,
`fn_under_test_does_X_when_Y`.

**Why:** the test name is the spec. When CI prints "FAILED
search_books_finds_by_title_and_ranks_by_bm25", you know what the
function is supposed to do without opening the test. "FAILED
finds_by_title" doesn't tell you *which* function or *how* it should
find.

**Pattern.** [db/src/books/tests.rs](../db/src/books/tests.rs) names
like `search_books_finds_by_title_and_ranks_by_bm25`,
`list_books_populates_formats_from_book_files`.

**Anti-pattern.** [db/src/journals/markdown/tests.rs](../db/src/journals/markdown/tests.rs)
names like `strips_script_tags` and `strips_event_handler_attributes`
elide the function-under-test convention entirely — both test `render`,
the function whose behavior they're checking, but neither name says so.
Compare `render_emits_del_for_strikethrough` a few lines above in the
same file, which correctly prefixes with the function it tests.

---

## Visibility & types

**Rule (visibility):** default `pub` is fine. Use `pub(crate)` only
when you have a specific reason a type *must* not leak.

**Why:** the module structure in this repo (small focused modules,
re-exported from `lib.rs`) makes `pub` read fine in practice. A
codebase-wide audit to narrow visibility would touch hundreds of
items without changing any behaviour. Not worth it.

**Rule (shared types):** anything used by 2+ crates lives in `shared/`.
Hoist when the second consumer appears.

**Rule (DB rows vs wire DTOs):** when the same record crosses the
HTTP/RPC boundary, it's fine to have a `db::Foo` and a `shared::Foo`
that look similar, with `From` / `Into` at the boundary. Don't try to
make one type serve both — the DB type wants sqlx derives, the wire
type wants serde, and the two can drift independently
(`#[serde(rename)]` on the wire side, columns added to the DB row
without a wire change).

---

## Mechanics

**Imports:** three blocks separated by blank lines — `std`, then
external crates, then `crate::` — each block alphabetical. This is a
convention enforced by review; we don't use a nightly-rustfmt
setting.

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use sqlx::SqlitePool;

use crate::books::row_to_ebook;
use crate::pool::init_db;
```

**`unwrap()` / `expect()`:** banned in production paths (see
[02-error-handling.md](../.claude/rules/02-error-handling.md)). Tests
use them freely.

**`unsafe`:** avoid it in production code. The current uses are test-only
environment guards and must carry `// SAFETY:` comments explaining the
serialization invariant. If a future production change needs unsafe,
document the invariant and call it out in the PR description.

---

## Out of scope

- TypeScript / Playwright — covered by
  [04-playwright.md](../.claude/rules/04-playwright.md).
- SQL migrations — covered by
  [.claude/rules/06-migrations.md](../.claude/rules/06-migrations.md).
