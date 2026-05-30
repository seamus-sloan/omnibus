import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import type { APIRequestContext } from "@playwright/test";

// F5.9-lite PR 5 — admin "Delete author" button on /authors/:id.
//
// The delete primitive (PR 2) inserts the author name into the
// `ignored_authors` blocklist, so a re-run of the test suite would
// keep finding the author missing even if we re-seed. Each test in
// this file picks a *different* fixture author; the one test that
// actually confirms a delete restores that author afterwards (see
// `restoreDeletedAuthor`) so the shared on-disk fixture library other
// specs read stays intact.

test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

async function fetchAuthorIdByName(
  request: APIRequestContext,
  name: string,
): Promise<number> {
  const resp = await request.get("/api/rpc/ebooks");
  expect(resp.status(), "GET /api/rpc/ebooks failed").toBe(200);
  const body = (await resp.json()) as {
    books: { creators: { name: string; id: number | null }[] }[];
  };
  for (const book of body.books) {
    const match = book.creators.find((c) => c.name === name);
    if (match?.id) return match.id;
  }
  throw new Error(`no indexed author named ${JSON.stringify(name)}`);
}

// Undo a confirmed delete so the shared fixture library other specs read
// stays intact.
//
// The E2E server runs against a *persistent* on-disk DB shared by every spec
// (unlike the per-process `sqlite::memory:` of the unit tests). `delete_author`
// drops the `books_authors_link` rows and durably blocklists the name in
// `ignored_authors`, and the indexer is incremental — a re-seed only re-parses
// files whose mtime changed, so it can't resurrect the author for an unchanged
// fixture EPUB. Left uncleaned, the un-linked book turns author-less for the
// rest of the run and breaks every later spec that asserts that author
// (`landing.spec` renders an empty author cell, etc.).
//
// We restore the link the reindex-proof, parallel-safe way: a per-book
// metadata override that re-asserts the original creator list at read time.
// Overrides are keyed by uuid and merged on read, so they survive the
// incremental re-seeds other specs trigger and never touch the shared library
// path (re-pointing settings would race parallel workers). The fixture table
// is the source of truth for each book's authors.
async function restoreDeletedAuthor(
  request: APIRequestContext,
  authorName: string,
): Promise<void> {
  const affected = FIXTURE_BOOKS.filter((b) => b.authors.includes(authorName));
  expect(
    affected.length,
    `restoreDeletedAuthor: no fixture book lists ${JSON.stringify(authorName)}`,
  ).toBeGreaterThan(0);

  for (const book of affected) {
    const uuid = await fetchBookUuidByTitle(request, book.title);
    const resp = await request.post("/api/rpc/ebook/overrides", {
      data: {
        uuid,
        overrides: {
          creators: book.authors.map((name) => ({ name, role: null, file_as: null })),
        },
      },
    });
    expect(
      resp.status(),
      `POST /api/rpc/ebook/overrides failed for ${book.title}`,
    ).toBe(200);
  }

  // Confirm the author is observable again before releasing the shared DB.
  await expect
    .poll(
      async () => {
        const r = await request.get("/api/rpc/ebooks");
        if (r.status() !== 200) return false;
        const body = (await r.json()) as {
          books: { creators: { name: string }[] }[];
        };
        return body.books.some((b) => b.creators.some((c) => c.name === authorName));
      },
      {
        message: `author ${JSON.stringify(authorName)} should be restored via override`,
        timeout: 10_000,
        intervals: [100, 200, 500, 1_000],
      },
    )
    .toBe(true);
}

test("renders Delete author button for admin viewers", async ({ page, request }) => {
  // Pick a fixture-only author so a failing test never wipes a real
  // author from the live library, and so other specs in this file can
  // use different authors without racing.
  const targetName = "Hedy Lamarr";
  const id = await fetchAuthorIdByName(request, targetName);
  await gotoReady(page, `/authors/${id}`);

  const deleteBtn = page.getByTestId("author-delete-btn");
  await expect(deleteBtn).toBeVisible();
});

test("clicking Delete opens the confirmation modal with the author name and book count", async ({
  page,
  request,
}) => {
  const targetName = "Sophie Germain";
  const id = await fetchAuthorIdByName(request, targetName);
  await gotoReady(page, `/authors/${id}`);

  await page.getByTestId("author-delete-btn").click();
  const modal = page.getByTestId("author-delete-modal");
  await expect(modal).toBeVisible();
  await expect(modal).toContainText(targetName);
  await expect(modal).toContainText(/un-link the author from \d+ books?/);

  await page.getByTestId("author-delete-cancel").click();
  await expect(modal).toHaveCount(0);
});

test("confirming Delete posts to rpc_delete_author and redirects to /authors", async ({
  page,
  request,
}) => {
  // Pick a single-book fixture author so the delete affects exactly one
  // book — keeps the assertion narrow.
  const targetName = "Joan Clarke";
  const id = await fetchAuthorIdByName(request, targetName);
  await gotoReady(page, `/authors/${id}`);

  await page.getByTestId("author-delete-btn").click();
  await expect(page.getByTestId("author-delete-modal")).toBeVisible();

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/author\/delete$/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("author-delete-confirm").click(),
  );

  // The redirect should land on the /authors index page. Match
  // trailing slash optional to accept either form.
  await expect(page).toHaveURL(/\/authors\/?$/);

  // The deleted author should no longer be linked to any book in the
  // /api/rpc/ebooks response — confirms the blocklist + link
  // teardown executed.
  const after = await request.get("/api/rpc/ebooks");
  const body = (await after.json()) as {
    books: { creators: { name: string }[] }[];
  };
  for (const book of body.books) {
    expect(book.creators.find((c) => c.name === targetName)).toBeUndefined();
  }

  await restoreDeletedAuthor(request, targetName);
});

test("surfaces an error and stays on the page when delete fails", async ({
  page,
  request,
}) => {
  const targetName = "Karen Sparck Jones";
  const id = await fetchAuthorIdByName(request, targetName);
  await gotoReady(page, `/authors/${id}`);

  await page.getByTestId("author-delete-btn").click();

  await page.route("**/api/rpc/author/delete", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/author\/delete$/,
      expectedStatus: 500,
    },
    async () => page.getByTestId("author-delete-confirm").click(),
  );

  // Modal stays open, error visible, URL unchanged.
  await expect(page.getByTestId("author-delete-modal")).toBeVisible();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/authors/${id}$`));

  // Cleanup: unroute and dismiss.
  await page.unroute("**/api/rpc/author/delete");
  await page.getByTestId("author-delete-cancel").click();
});
