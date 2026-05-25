import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import type { APIRequestContext } from "@playwright/test";

// F5.9-lite PR 5 — admin "Delete author" button on /authors/:id.
//
// The delete primitive (PR 2) inserts the author name into the
// `ignored_authors` blocklist, so a re-run of the test suite would
// keep finding the author missing even if we re-seed. Each test in
// this file picks a *different* fixture author and cleans up by
// DELETE-ing the blocklist row at the end, so subsequent runs start
// from a clean state.

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

// Re-seed the library after each delete so the deleted author re-appears
// on disk (the EPUB is still there) and the ignored_authors guard is the
// only thing keeping the row absent. Then DELETE the blocklist row so
// the next reindex re-creates the author cleanly. Without this cleanup
// every subsequent test run would fail because the seeded fixture's
// author is permanently blocklisted.
async function clearBlocklistAndReseed(
  request: APIRequestContext,
  authorName: string,
): Promise<void> {
  // No public RPC for `DELETE FROM ignored_authors` exists yet — the
  // F5.9-lite plan defers that to a follow-up admin tool. For now we
  // rely on the in-memory DB being fresh between full test suite runs;
  // this helper is a stub so the test file documents the cleanup
  // intent and can switch to the real call once it exists.
  void request;
  void authorName;
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

  await clearBlocklistAndReseed(request, targetName);
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
