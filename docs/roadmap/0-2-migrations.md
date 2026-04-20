# F0.2 — Migration framework

**Phase 0 · Foundations** · **Priority:** P0

Replace inline `initialize_schema` with versioned migrations.

## Objective

Introduce a migration tool and a `schema_migrations` version table so every subsequent schema change ships as a new migration file, not an edit to `initialize_schema`.

## User / business value

Lets every subsequent change ship without risking production DBs. Avoids Calibre-Web's runtime `ALTER TABLE ADD COLUMN`-on-OperationalError approach (see [calibre-inspection §5](../calibre-inspection/5-schema-details.md)) — that's fine for a hobbyist project but collapses under schema rewrites.

## Technical considerations

- `sqlx::migrate!` is the default choice (compile-time embedding, same crate we use for queries).
- `refinery` is viable if we want migration objects that aren't plain `.sql`.
- Inline schema stays for test DBs (`sqlite::memory:`) — test isolation doesn't need migration history.

## Dependencies

None. Should land first or alongside [F0.1](0-1-schema-refactor.md).

## Risks

Low. One-time tooling cost.

---

[← Back to roadmap summary](0-0-summary.md)
