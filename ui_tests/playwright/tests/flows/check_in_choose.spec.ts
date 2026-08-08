import type { APIRequestContext, Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import { bookTile, selectShelfInGallery } from "../utils/shelves";

// The 3c "not in your library" chooser: own it (creates a fileless book plus
// its first physical copy) or wishlist it (creates a fileless book tracked
// only on the caller's Wishlist shelf). The ISBN resolve is mocked — same
// provider-network rationale as the rest of the check-in suite — but the
// write each button triggers is real: both create a brand-new book with a
// title/author no other spec's fixtures or assertions ever reference, so
// there is nothing shared to race or leak. That's what lets these two tests
// verify the actual acceptance criteria (visible in All Books; visible on
// the owner's Wishlist shelf, including to a second, non-owner user) rather
// than a mocked stand-in for them. Both clean up what they created, in a
// `finally` so a mid-test assertion failure still doesn't leak state.

const ISBN = "9780441013593";
const VIEWER_USER = "checkinviewer";
const VIEWER_PASSWORD = "checkin-viewer-pw-00";

// A fileless book only reaches the landing list when a library path is set
// (`list_books_page` short-circuits on an empty path set), so this spec has
// to seed rather than inherit whichever parallel spec happened to seed first.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

function candidate(
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    isbn13: "9990000000001",
    title: "An Untitled Placeholder",
    authors: ["Ada Placeholder"],
    year: "2020",
    pages: null,
    publisher: null,
    description: null,
    cover_url: null,
    series: null,
    first_publish_year: null,
    source: "open_library",
    ...over,
  };
}

async function mockJsonPost(
  page: Page,
  url: string | RegExp,
  body: unknown,
): Promise<void> {
  await page.route(url, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(body),
        })
      : route.continue(),
  );
}

/** Mock the resolve so a typed ISBN opens the 3c chooser with `online`. */
async function reachChoose(
  page: Page,
  online: Record<string, unknown>,
): Promise<void> {
  await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
    kind: "not_in_library",
    online,
  });
  await gotoReady(page, "/check-in");
  await page.getByTestId("check-in-isbn").fill(ISBN);
  await expectMutation(
    page,
    { method: "POST", url: /\/api\/rpc\/scan\/resolve$/, expectedStatus: 200 },
    async () => page.getByTestId("check-in-submit").click(),
  );
  await expect(page.getByTestId("check-in-choose")).toBeVisible();
}

/** Provision the dedicated viewer user via the admin API; 409 = already there. */
async function ensureViewerUser(request: APIRequestContext): Promise<void> {
  const resp = await request.post("/api/users", {
    data: {
      username: VIEWER_USER,
      password: VIEWER_PASSWORD,
      permissions: {
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
      },
    },
  });
  expect([201, 409]).toContain(resp.status());
}

/** Log the dedicated viewer user in through the login UI on a cookie-less page. */
async function logInAsViewer(page: Page): Promise<void> {
  await gotoReady(page, "/login");
  await page.getByLabel("Username").fill(VIEWER_USER);
  await page.getByLabel("Password").fill(VIEWER_PASSWORD);
  await expectMutation(
    page,
    { method: "POST", url: "/api/auth/login", expectedStatus: 200 },
    async () => page.getByRole("button", { name: "Log in" }).click(),
  );
  await expect(page).toHaveURL(/\/$/);
}

/** The caller's own Wishlist shelf, which `provision_wishlist_shelf` guarantees. */
async function fetchWishlistShelf(
  request: APIRequestContext,
): Promise<{ id: number; name: string }> {
  const resp = await request.get("/api/rpc/shelves");
  expect(resp.status()).toBe(200);
  const shelves = (await resp.json()) as {
    id: number;
    kind: string;
    name: string;
  }[];
  const wishlist = shelves.find((s) => s.kind === "wishlist");
  if (!wishlist) {
    throw new Error("the seeded admin has no Wishlist shelf");
  }
  return { id: wishlist.id, name: wishlist.name };
}

test("adds a not-in-library book to the physical collection, shows it in All Books, then removes it as the last copy", async ({
  page,
  request,
}) => {
  const title = `E2E Physical-Only ${Date.now()}`;
  const online = candidate({
    isbn13: "9990000000011",
    title,
    authors: ["Ada Placeholder"],
  });
  await reachChoose(page, online);

  const { response } = await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/scan/physical-only",
      expectedStatus: 200,
    },
    async () => page.getByTestId("check-in-own-it").click(),
  );
  const { book_uuid: uuid } = (await response.json()) as {
    book_uuid: string;
  };

  await expect(page.getByTestId("check-in-success")).toContainText(
    "In your physical collection",
  );
  await expect(page.getByTestId("check-in-success")).toContainText(title);

  let bookDeleted = false;
  try {
    // Real read: F-Physical-Check-In's visibility rule surfaces a fileless
    // book once it holds a physical copy, so it belongs in All Books.
    await gotoReady(page, "/");
    await expect(bookTile(page, title)).toBeVisible();

    await gotoReady(page, `/books/${uuid}`);
    await expect(
      page.getByRole("heading", { level: 1, name: title }),
    ).toBeVisible();
    await expect(page.getByTestId("physical-pill")).toBeVisible();

    // The single copy of a fileless book gets the remove-or-wishlist choice,
    // not the plain confirm (`physical_collection.spec.ts` covers that one
    // on a file-backed book). Deleting it fires two sequential writes —
    // the copy, then the now-copy-less fileless book — so both waiters are
    // armed before the click rather than using `expectMutation` twice in a
    // row, which could register the second waiter after that request had
    // already fired.
    const copyDelete = page.waitForRequest(
      (r) =>
        r.method() === "POST" &&
        r.url().includes("/api/rpc/physical/copies/delete"),
    );
    const bookDelete = page.waitForRequest(
      (r) =>
        r.method() === "POST" &&
        r.url().includes("/api/rpc/physical/book/delete"),
    );
    await page.getByTestId("copy-delete").click();
    await expect(page.getByTestId("last-copy-modal")).toBeVisible();
    await page.getByTestId("last-copy-remove").click();

    const [copyReq, bookReq] = await Promise.all([copyDelete, bookDelete]);
    const copyResp = await copyReq.response();
    const bookResp = await bookReq.response();
    expect(copyResp?.status()).toBe(200);
    expect(bookResp?.status()).toBe(200);
    // `rpc_delete_fileless_book(uuid)` — the body this call must carry is
    // known (the book uuid this test created); `rpc_delete_physical_copy`
    // takes a `copy_id` this test never queried, so that half stays a
    // status-only assertion.
    expect(bookReq.postDataJSON()).toEqual({ uuid });
    bookDeleted = true;
  } finally {
    // Best-effort: only fires if an assertion above threw before the UI
    // delete completed, so the book doesn't leak into another spec's
    // library. Skipped when the UI delete already succeeded — a redundant
    // call against an already-deleted book is a business-logic error whose
    // exact status this suite has no other test to pin down, and asserting
    // a guessed code would make cleanup itself a source of flakiness.
    if (!bookDeleted) {
      const cleanupResp = await request
        .post("/api/rpc/physical/book/delete", { data: { uuid } })
        .catch(() => null);
      expect
        .soft(cleanupResp?.status(), "cleanup delete of leaked book")
        .toBe(200);
    }
  }

  await gotoReady(page, "/");
  await expect(bookTile(page, title)).toHaveCount(0);
});

test("adds a not-in-library book to the wishlist, shows it on the owner's Wishlist shelf, and a second user can view the public shelf", async ({
  page,
  request,
  browser,
}) => {
  const title = `E2E Wishlist Pick ${Date.now()}`;
  const online = candidate({
    isbn13: "9990000000012",
    title,
    authors: ["Grace Placeholder"],
  });
  await reachChoose(page, online);

  const { response } = await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/scan/wishlist", expectedStatus: 200 },
    async () => page.getByTestId("check-in-wishlist").click(),
  );
  const { book_uuid: uuid } = (await response.json()) as {
    book_uuid: string;
  };

  await expect(page.getByTestId("check-in-success")).toContainText(
    "On your wishlist",
  );
  await expect(page.getByTestId("check-in-success")).toContainText(title);

  const wishlist = await fetchWishlistShelf(request);

  try {
    // The Wishlist is reached the way a user reaches any shelf: its tile in
    // the landing gallery, which filters the landing list in place.
    await gotoReady(page, "/");
    await selectShelfInGallery(page, wishlist.id, wishlist.name);
    await expect(bookTile(page, title)).toBeVisible();

    // Public by design (`provision_wishlist_shelf`) — a second, freshly
    // provisioned, non-admin user can view it too via a cookie-less context,
    // mirroring `hidden_formats.spec.ts`'s per-user isolation pattern.
    await ensureViewerUser(request);
    const context = await browser.newContext({
      storageState: { cookies: [], origins: [] },
    });
    const viewerPage = await context.newPage();
    try {
      await logInAsViewer(viewerPage);
      await gotoReady(viewerPage, "/");
      await selectShelfInGallery(viewerPage, wishlist.id, wishlist.name);
      await expect(bookTile(viewerPage, title)).toBeVisible();
      await expect(viewerPage.getByTestId("lib-page-error")).toHaveCount(0);
    } finally {
      await context.close();
    }
  } finally {
    // `scan/wishlist` creates a new fileless book row, not just a
    // `wishlist_entries` row — removing only the wishlist entry leaves that
    // invisible-but-real book behind for later specs. Delete the book
    // outright instead: `purge_book` already cascades `wishlist_entries`
    // (and `physical_copies`) along with it, so this one call is complete
    // cleanup on its own.
    const cleanupResp = await request
      .post("/api/rpc/physical/book/delete", { data: { uuid } })
      .catch(() => null);
    expect
      .soft(cleanupResp?.status(), "cleanup delete of leaked wishlist book")
      .toBe(200);
  }

  await gotoReady(page, "/");
  await selectShelfInGallery(page, wishlist.id, wishlist.name);
  await expect(bookTile(page, title)).toHaveCount(0);
});

test("surfaces an error when adding a physical-only book fails", async ({
  page,
}) => {
  const online = candidate({
    isbn13: "9990000000013",
    title: "A Book That Never Gets Added",
  });
  await reachChoose(page, online);

  await page.route("**/api/rpc/scan/physical-only", (route) =>
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
    {
      method: "POST",
      url: "/api/rpc/scan/physical-only",
      expectedStatus: 500,
    },
    async () => page.getByTestId("check-in-own-it").click(),
  );

  await expect(page.getByTestId("check-in-error")).toBeVisible();
  await expect(page.getByTestId("check-in-choose")).toBeVisible();
});

test("surfaces an error when adding to the wishlist fails", async ({
  page,
}) => {
  const online = candidate({
    isbn13: "9990000000014",
    title: "A Book That Never Gets Wishlisted",
  });
  await reachChoose(page, online);

  await page.route("**/api/rpc/scan/wishlist", (route) =>
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
    { method: "POST", url: "/api/rpc/scan/wishlist", expectedStatus: 500 },
    async () => page.getByTestId("check-in-wishlist").click(),
  );

  await expect(page.getByTestId("check-in-error")).toBeVisible();
  await expect(page.getByTestId("check-in-choose")).toBeVisible();
});
