import type { APIRequestContext, Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// "I already have this book" — the escape hatch for a copy no rung of the
// matching ladder can place (issue #1790). The real case: a provider answers a
// print barcode with a pre-release placeholder title that shares nothing with
// the library book, so neither the exact-ISBN nor the (title, author) rung can
// bridge it and the flow used to mint a silent duplicate.
//
// The ISBN resolve is mocked — same provider-network rationale as the rest of
// the check-in suite — but everything downstream of the picker is real: the
// library search, the check-in write, and the re-scan that must now land on
// the linked book. That's what lets this spec assert the actual acceptance
// criteria rather than a stand-in for them.
//
// TARGET is reserved for this spec in `fixtures/epubs.ts`: the filed copy is a
// library-wide row, and the ISBN below stays bound to the book on the
// exact-identifier rung even after the copy is deleted.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "polyglot-2")!;
/** A distinctive word from TARGET's title, so the FTS query matches it alone. */
const TARGET_QUERY = "Cuentos";
/** Valid ISBN-13 owned by this spec — no fixture publishes it. */
const ISBN = "9781402894626";
/** Unique per run, so a leaked copy from an earlier one is never mistaken. */
const NOTE = `Signed paperback ${Date.now()}`;

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

function placeholder(): Record<string, unknown> {
  return {
    isbn13: ISBN,
    // The shape of the bug: a title with no word in common with the shelf copy.
    title: "Untitled Placeholder Release (Deluxe Limited Edition)",
    authors: ["Unknown Placeholder"],
    year: null,
    pages: null,
    publisher: null,
    description: null,
    cover_url: null,
    series: null,
    first_publish_year: null,
    source: "open_library",
  };
}

/** Mock the resolve so a typed ISBN opens the 3c chooser with a placeholder. */
async function reachChoose(page: Page): Promise<void> {
  await page.route(/\/api\/rpc\/scan\/resolve$/, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            kind: "not_in_library",
            online: placeholder(),
          }),
        })
      : route.continue(),
  );
  await gotoReady(page, "/check-in");
  await page.getByTestId("check-in-isbn").fill(ISBN);
  await expectMutation(
    page,
    { method: "POST", url: /\/api\/rpc\/scan\/resolve$/, expectedStatus: 200 },
    async () => page.getByTestId("check-in-submit").click(),
  );
  await expect(page.getByTestId("check-in-choose")).toBeVisible();
}

/**
 * Open the picker and search the real library. The palette search is a read
 * that POSTs (a Dioxus server function), so it goes through `expectMutation`
 * for its status the same way the flow's other RPC reads do.
 */
async function searchLibrary(page: Page, query: string): Promise<void> {
  await page.getByTestId("check-in-link-existing").click();
  await expect(page.getByTestId("check-in-link")).toBeVisible();
  await page.getByLabel("Title or author").fill(query);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/search-palette",
      expectedBody: { q: query },
      expectedStatus: 200,
    },
    async () => page.getByTestId("check-in-link-submit").click(),
  );
}

/** The library-wide copies of a book, via the RPC book detail reads. */
async function fetchCopies(
  request: APIRequestContext,
  uuid: string,
): Promise<{ id: number; isbn: string | null; note: string | null }[]> {
  const resp = await request.post("/api/rpc/physical/copies", {
    data: { uuid },
  });
  expect(resp.status()).toBe(200);
  return (await resp.json()) as {
    id: number;
    isbn: string | null;
    note: string | null;
  }[];
}

/**
 * Delete the copies this spec filed, leaving the fixture as it was. Scoped to
 * this run's note so a concurrently-running test's copy is never swept.
 */
async function deleteOwnCopies(
  request: APIRequestContext,
  uuid: string,
): Promise<void> {
  const mine = (await fetchCopies(request, uuid)).filter(
    (c) => c.note === NOTE,
  );
  for (const copy of mine) {
    const resp = await request.post("/api/rpc/physical/copies/delete", {
      data: { copy_id: copy.id },
    });
    expect.soft(resp.status(), "cleanup delete of the filed copy").toBe(200);
  }
}

test("renders the link-to-an-existing-book picker", async ({ page }) => {
  await reachChoose(page);
  // The affordance sits on the chooser itself — the screen that would
  // otherwise only offer "add as new" and create the duplicate.
  await expect(page.getByTestId("check-in-link-existing")).toBeVisible();

  await page.getByTestId("check-in-link-existing").click();
  await expectNavVisible(page);

  await expect(page.getByTestId("check-in-link")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Which book is this?" }),
  ).toBeVisible();
  await expect(page.getByLabel("Title or author")).toBeVisible();
  // Nothing typed yet: the search can't fire and no stale empty state shows.
  await expect(page.getByTestId("check-in-link-submit")).toBeDisabled();
  await expect(page.getByTestId("check-in-link-empty")).toHaveCount(0);
  await expect(page.getByTestId("check-in-link-back")).toBeVisible();
});

test("links a scanned copy to a library book without creating a second book", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  await reachChoose(page);
  await searchLibrary(page, TARGET_QUERY);

  const result = page.getByTestId("check-in-link-result");
  await expect(result).toHaveCount(1);
  await expect(result).toContainText(TARGET.title);
  await result.click();

  // The pick lands on the ordinary in-library confirm, so the copy is filed
  // through the one existing write path — note field included.
  await expect(page.getByTestId("check-in-confirm")).toBeVisible();
  await page.getByTestId("check-in-note").fill(NOTE);

  try {
    const { response } = await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/scan/check-in",
        expectedBody: { req: { book_uuid: uuid, isbn: ISBN, note: NOTE } },
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-confirm-submit").click(),
    );
    // AC2: the write lands on the *existing* book, not a freshly minted one.
    expect(await response.json()).toEqual({ book_uuid: uuid });

    await expect(page.getByTestId("check-in-success")).toContainText(
      "In your physical collection",
    );
    await expect(page.getByTestId("check-in-success")).toContainText(
      TARGET.title,
    );

    // AC3a: the chosen book's detail page shows the copy.
    await gotoReady(page, `/books/${uuid}`);
    await expect(page.getByTestId("physical-pill")).toBeVisible();
    await expect(
      page.getByTestId("physical-copy-card").filter({ hasText: NOTE }),
    ).toBeVisible();

    // AC3b: a re-scan of the same barcode now resolves to the linked book on
    // the exact-identifier rung — no provider round trip, no second chooser.
    const rescan = await request.post("/api/rpc/scan/resolve", {
      data: { req: { isbn: ISBN } },
    });
    expect(rescan.status()).toBe(200);
    expect(await rescan.json()).toMatchObject({
      kind: "already_owned",
      book: { uuid },
    });
  } finally {
    await deleteOwnCopies(request, uuid);
  }
});

test("surfaces an error when the link check-in fails, keeping the confirm", async ({
  page,
}) => {
  await reachChoose(page);
  await searchLibrary(page, TARGET_QUERY);
  await page.getByTestId("check-in-link-result").first().click();
  await expect(page.getByTestId("check-in-confirm")).toBeVisible();

  await page.route("**/api/rpc/scan/check-in", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 500,
          contentType: "text/plain",
          body: "forced failure",
        })
      : route.continue(),
  );
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/scan/check-in", expectedStatus: 500 },
    async () => page.getByTestId("check-in-confirm-submit").click(),
  );

  await expect(page.getByTestId("check-in-error")).toBeVisible();
  // The reader stays on the confirm with the picked book, free to retry.
  await expect(page.getByTestId("check-in-confirm")).toBeVisible();
});

test("says so when the library has no match, and Back restores the chooser", async ({
  page,
}) => {
  await reachChoose(page);
  await searchLibrary(page, "zzzz no such book in this library zzzz");

  await expect(page.getByTestId("check-in-link-empty")).toBeVisible();
  await expect(page.getByTestId("check-in-link-results")).toHaveCount(0);

  // Back returns the reader to the outcome they left, still holding the
  // resolved book — not to a blank ISBN field.
  await page.getByTestId("check-in-link-back").click();
  await expect(page.getByTestId("check-in-choose")).toBeVisible();
  await expect(page.getByTestId("check-in-choose")).toContainText(
    "Untitled Placeholder Release",
  );
});
