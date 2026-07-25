import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Physical-collection + wishlist book-detail UI (issue #1186). The physical /
// wishlist RPCs are `/api/rpc/physical/*` server functions returning plain
// JSON of their value (same envelope the check-in scanner's key-status mock
// relies on), so every state is driven with `page.route` fulfilments. That
// keeps the suite race-free: nothing here mutates library-wide physical copies
// or the admin's real wishlist, so no parallel spec's assertions are disturbed
// and no extra book leaks into any count.

// A plain single-format ebook to land on. Which book is immaterial — the
// physical endpoints are mocked — but a real uuid exercises the real SSR +
// hydration path the panel loads under.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "standalone-island")!;

type CopyMock = {
  id: number;
  book_uuid: string;
  isbn: string | null;
  added_by_user_id: number | null;
  checked_in_at: number;
  note: string | null;
};

type WishlistMock = {
  id: number;
  user_id: number;
  book_uuid: string;
  added_at: number;
  source: "scan" | "detail" | "manual";
};

const SAMPLE_COPY: CopyMock = {
  id: 42,
  book_uuid: "uuid",
  isbn: "9780262033848",
  added_by_user_id: 1,
  checked_in_at: 1_700_000_000,
  note: "Trade paperback, MIT Press",
};

const SAMPLE_WISHLIST: WishlistMock = {
  id: 7,
  user_id: 1,
  book_uuid: "uuid",
  added_at: 1_700_000_000,
  source: "detail",
};

// Fulfil the two on-mount load reads (copies + wishlist) so the panel settles
// into a deterministic state regardless of ambient server data.
async function mockPhysicalLoad(
  page: Page,
  copies: CopyMock[],
  wishlist: WishlistMock | null,
): Promise<void> {
  await page.route("**/api/rpc/physical/copies", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(copies),
    }),
  );
  await page.route("**/api/rpc/physical/wishlist/get", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(wishlist),
    }),
  );
}

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test("renders the physical panel with the add-to-wishlist default state", async ({
  page,
  request,
}) => {
  // No mocks: a plain ebook with no copies and no wishlist entry loads the
  // real RPCs and settles into the "add to physical wishlist" affordance.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);
  await expectNavVisible(page);

  await expect(page.getByTestId("bd-physical-panel")).toBeVisible();
  await expect(page.getByTestId("add-to-wishlist")).toBeVisible();
  // Nothing checked in, so no physical pill.
  await expect(page.getByTestId("physical-pill")).toBeHidden();
});

test("adds then removes a wishlist entry", async ({ page, request }) => {
  await mockPhysicalLoad(page, [], null);
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await expect(page.getByTestId("add-to-wishlist")).toBeVisible();

  await page.route("**/api/rpc/physical/wishlist/add", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(SAMPLE_WISHLIST),
    }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/physical/wishlist/add",
      expectedStatus: 200,
    },
    async () => page.getByTestId("add-to-wishlist").click(),
  );

  await expect(page.getByTestId("wishlist-pill")).toBeVisible();
  await expect(page.getByTestId("wishlist-card")).toBeVisible();
  await expect(page.getByTestId("find-a-copy")).toBeVisible();

  await page.route("**/api/rpc/physical/wishlist/remove", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: "null",
    }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/physical/wishlist/remove",
      expectedStatus: 200,
    },
    async () => page.getByTestId("wishlist-remove").click(),
  );

  await expect(page.getByTestId("add-to-wishlist")).toBeVisible();
});

test("surfaces an error when the wishlist add fails", async ({
  page,
  request,
}) => {
  await mockPhysicalLoad(page, [], null);
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.route("**/api/rpc/physical/wishlist/add", (route) =>
    route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "boom",
    }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/physical/wishlist/add",
      expectedStatus: 500,
    },
    async () => page.getByTestId("add-to-wishlist").click(),
  );

  await expect(page.getByTestId("physical-error")).toBeVisible();
  // The optimistic state reverted — still offering to add.
  await expect(page.getByTestId("add-to-wishlist")).toBeVisible();
});

test("shows the physical copy card for a checked-in book", async ({
  page,
  request,
}) => {
  await mockPhysicalLoad(page, [SAMPLE_COPY], null);
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await expect(page.getByTestId("physical-pill")).toBeVisible();
  await expect(page.getByTestId("physical-pill")).toHaveText(
    "In your physical collection",
  );
  const card = page.getByTestId("physical-copy-card");
  await expect(card).toBeVisible();
  await expect(card).toContainText("Trade paperback, MIT Press");
  await expect(card).toContainText("9780262033848");
});

test("edits a copy note", async ({ page, request }) => {
  await mockPhysicalLoad(page, [SAMPLE_COPY], null);
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("copy-edit-note").click();
  await page.getByLabel("Edition note").fill("Hardcover, first printing");

  await page.route("**/api/rpc/physical/copies/note", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ...SAMPLE_COPY,
        note: "Hardcover, first printing",
      }),
    }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/physical/copies/note",
      expectedStatus: 200,
    },
    async () => page.getByTestId("copy-note-save").click(),
  );

  await expect(page.getByTestId("physical-copy-card")).toContainText(
    "Hardcover, first printing",
  );
});

test("deletes a copy through the I-sold-it confirm", async ({
  page,
  request,
}) => {
  await mockPhysicalLoad(page, [SAMPLE_COPY], null);
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // A file-backed book, so a copy delete is the plain confirm — not the
  // last-copy remove-or-wishlist prompt.
  await page.getByTestId("copy-delete").click();
  await expect(page.getByTestId("copy-delete-modal")).toBeVisible();

  await page.route("**/api/rpc/physical/copies/delete", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: "null",
    }),
  );
  // Deleting the last copy bumps the page refresh, which refetches the book.
  await page.route("**/api/rpc/physical/copies", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: "[]" }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/physical/copies/delete",
      expectedStatus: 200,
    },
    async () => page.getByTestId("copy-delete-confirm").click(),
  );

  await expect(page.getByTestId("physical-copy-card")).toBeHidden();
});
