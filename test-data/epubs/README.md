# Playwright EPUB fixtures

This directory holds the EPUBs the Playwright landing-page spec seeds against.
The test-only `seedLibrary()` helper points the running server at this absolute
path and waits for the indexer to surface the same number of books listed in
`ui_tests/playwright/tests/fixtures/epubs.ts`.

## Contents

```
generated/    — synthetic EPUBs produced by tools/make_epub.ts (committed)
publicDomain/ — real EPUBs from Project Gutenberg / Standard Ebooks
```

The seeder points the server at `test-data/epubs/` (this directory). The
scanner recurses, so both subdirectories load in a single seed call.

## Synthetic vs. public-domain

The synthetic EPUBs cover every metadata column the landing page renders
(title, authors, publisher, date, language, series + index, cover / no-cover)
because we control their OPFs end-to-end. The public-domain EPUBs exercise the
OPF parser against real-world files we don't control; their expected metadata
is pinned in `db/tests/public_domain_epubs.rs` so a parser change that subtly
drops a field surfaces there before Playwright sees it.

## Single source of truth for expected metadata

`ui_tests/playwright/tests/fixtures/epubs.ts` exports `FIXTURE_BOOKS`, the
table the spec asserts against. The generator inputs in
`ui_tests/playwright/tools/make_epub.ts` and that table must stay in sync —
the `slug` field on each fixture matches the row testid the landing page emits
for that file.

## Regenerating

The generated EPUBs are deterministic (fixed timestamps, fixed deflate level)
so re-running the generator produces byte-identical output unless the inputs
change. You only need to regenerate when you edit `make_epub.ts`:

```bash
cd ui_tests/playwright
npx tsx tools/make_epub.ts
```

Then commit the updated `.epub` files alongside the generator change.
