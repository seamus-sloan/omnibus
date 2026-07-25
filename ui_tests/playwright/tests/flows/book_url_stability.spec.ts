import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

/**
 * Regression coverage for the "bookmarked `/books/:id` URL goes 404 after
 * the next reindex" failure mode that motivated the uuid-URL switch.
 *
 * `books.id` is `AUTOINCREMENT` and renumbers on every `DELETE+INSERT`
 * pass through the indexer, so a URL captured before a reindex used to
 * resolve to a different (or no) book afterward. The route now keys on
 * `books.uuid` — a deterministic UUIDv5 of `(library_path, filename)` —
 * which survives reindexes, re-installs, and any other churn.
 *
 * The spec captures a uuid URL, kicks a reindex via the same settings
 * POST the seed flow uses (which the indexer's incremental sync handles
 * via the Backfill bucket on the first post-migration pass), and
 * re-fetches the same URL. The detail heading must still match.
 */
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

test("bookmarked /books/:uuid still resolves after a reindex", async ({
  page,
  request,
}) => {
  // Capture the uuid before the reindex. This is the URL a user would
  // have bookmarked.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  // First visit — pre-reindex. Detail heading matches the fixture.
  await gotoReady(page, `/books/${uuid}`);
  await expect(
    page.getByRole("heading", { level: 1, name: TARGET.title }),
  ).toBeVisible();

  // Kick a fresh reindex by re-seeding against the same library path.
  // The indexer's diff/sync path treats every book as either Backfill
  // (first run after the fs-metadata migration) or Unchanged, and `id`
  // is preserved either way. UUID is deterministic from path+filename,
  // so the captured URL stays valid.
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);

  // Second visit — same URL, post-reindex. Must still render the same
  // book; "Book not found" failure means the route lookup is no longer
  // stable across reindexes.
  await gotoReady(page, `/books/${uuid}`);
  await expect(
    page.getByRole("heading", { level: 1, name: TARGET.title }),
  ).toBeVisible();
  await expect(page.getByText("Book not found.")).toHaveCount(0);
});
