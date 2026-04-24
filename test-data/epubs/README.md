# Playwright EPUB fixtures

This directory holds the EPUBs the Playwright landing-page spec seeds against.
The test-only `seedLibrary()` helper points the running server at this absolute
path and waits for the indexer to surface the same number of books listed in
`ui_tests/playwright/tests/fixtures/epubs.ts`.

## Contents

```
generated/   — synthetic EPUBs produced by tools/make_epub.ts (committed)
```

## Why synthetic instead of public-domain text?

The plan originally called for a small set of Project Gutenberg EPUBs alongside
the synthetic ones, but synthetic EPUBs already give us total control over every
metadata column the landing page renders (title, authors, publisher, date,
language, series + index, cover/no-cover) without depending on what Gutenberg
happens to ship in their OPF. Add real EPUBs here later if a test needs to
exercise the parser against real-world data; the synthetic set is sufficient for
the landing-page contract.

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
