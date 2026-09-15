import type { APIRequest, APIRequestContext, Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import { bookTile, createShelf, openShelfFromIndex } from "../utils/shelves";

// The web shelf page, reached the way a reader reaches it: from its card on
// the `/shelves` index. Every membership write here lands on a shelf this file
// made; the fixture books those shelves hold (Alpha, Beta in the Series) are
// only ever read.
//
// The suite's admin may change every shelf except a Wishlist, so the locked
// states come from the two things it can't change: its own Wishlist, and —
// through a second, non-admin reader on a cookie-less context — a shelf that
// reader doesn't own.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const READER_PASSWORD = "shelf-reader-pw-0001";

const editButton = (page: Page) => page.getByTestId("shelf-edit");
const addButton = (page: Page) => page.getByTestId("shelf-add-books");
const moreButton = (page: Page) => page.getByTestId("shelf-actions");
const lockNote = (page: Page) => page.getByTestId("shelf-lock-reason");
const grid = (page: Page) => page.getByTestId("shelf-grid");
const title = (page: Page, name: string) =>
  page.getByRole("heading", { level: 1, name });

/** Create a non-admin reader through the admin API and return their id. */
async function createReader(
  request: APIRequestContext,
  username: string,
): Promise<number> {
  const resp = await request.post("/api/users", {
    data: {
      username,
      password: READER_PASSWORD,
      permissions: {
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
      },
    },
  });
  expect(resp.status(), `POST /api/users failed for ${username}`).toBe(201);
  return ((await resp.json()) as { id: number }).id;
}

/** Sign `username` in through the login form on a cookie-less page. */
async function logIn(page: Page, username: string): Promise<void> {
  await gotoReady(page, "/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(READER_PASSWORD);
  await expectMutation(
    page,
    { method: "POST", url: "/api/auth/login", expectedStatus: 200 },
    async () => page.getByRole("button", { name: "Log in" }).click(),
  );
  await expect(page).toHaveURL(/\/$/);
}

/** Create a shelf owned by `username`, on a bearer session of its own. */
async function createShelfAs(
  api: APIRequest,
  baseURL: string,
  username: string,
  body: Record<string, unknown>,
): Promise<number> {
  const ctx = await api.newContext({ baseURL });
  try {
    const login = await ctx.post("/api/auth/login", {
      data: { username, password: READER_PASSWORD, client_kind: "bearer" },
    });
    expect(login.status(), `bearer login failed for ${username}`).toBe(200);
    const { token } = (await login.json()) as { token: string };
    const resp = await ctx.post("/api/rpc/shelves/create", {
      headers: { Authorization: `Bearer ${token}` },
      data: {
        req: {
          description: null,
          visibility: "private",
          match_mode: null,
          rules: [],
          book_uuids: [],
          ...body,
        },
      },
    });
    expect(resp.status(), `shelf create failed for ${username}`).toBe(200);
    return ((await resp.json()) as { id: number }).id;
  } finally {
    await ctx.dispose();
  }
}

/** The admin's own Wishlist — the one shelf it can see but never change. */
async function ownWishlistId(request: APIRequestContext): Promise<number> {
  const me = (await (await request.get("/api/auth/me")).json()) as {
    id: number;
  };
  const shelves = (await (await request.get("/api/shelves")).json()) as {
    id: number;
    kind: string;
    owner_user_id: number;
  }[];
  const wishlist = shelves.find(
    (s) => s.kind === "wishlist" && s.owner_user_id === me.id,
  );
  expect(wishlist, "the suite's admin has a Wishlist").toBeTruthy();
  return wishlist!.id;
}

test("renders the shelf page layout", async ({ page, request }) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Detail Layout ${Date.now()}`;
  const id = await createShelf(request, {
    kind: "manual",
    name,
    book_uuids: [alpha],
  });

  await openShelfFromIndex(page, id);

  await expectNavVisible(page);
  await expect(title(page, name)).toBeVisible();
  await expect(page.getByTestId("shelf-kicker")).toContainText(
    "Hand-picked shelf",
  );
  await expect(page.getByTestId("shelf-owner")).toContainText("Your shelf");
  await expect(addButton(page)).toBeEnabled();
  await expect(editButton(page)).toBeEnabled();
  await expect(moreButton(page)).toBeEnabled();
  await expect(lockNote(page)).toHaveCount(0);
  await expect(grid(page)).toContainText("Alpha");
  await expect(page.getByTestId("shelf-back")).toHaveAttribute(
    "href",
    "/shelves",
  );
});

test("opens a book from the shelf", async ({ page, request }) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Book ${Date.now()}`,
    book_uuids: [alpha],
  });

  await openShelfFromIndex(page, id);
  await bookTile(page, "Alpha").click();

  await expect(page).toHaveURL(new RegExp(`/books/${alpha}$`));
});

test("adds a book to a hand-picked shelf", async ({ page, request }) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const shelfName = `E2E Detail Add ${Date.now()}`;
  const id = await createShelf(request, { kind: "manual", name: shelfName });

  await openShelfFromIndex(page, id);
  await expect(page.getByTestId("shelf-empty")).toContainText(
    "No books on this shelf yet.",
  );

  await addButton(page).click();
  const modal = page.getByTestId("add-books-modal");
  // The dialog names the shelf it is adding to, and can't add nothing.
  await expect(
    modal.getByRole("heading", { level: 2, name: shelfName }),
  ).toBeVisible();
  await expect(modal.getByTestId("add-books-submit")).toBeDisabled();

  await modal.getByTestId("add-books-search").fill("Alpha");
  const tile = modal.getByTestId(`picker-tile-${alpha}`);
  // A card, not a bare cover: it names the book, so a coverless library is
  // still pickable. The tile is a <button>, which shrink-wraps instead of
  // stretching to its grid track — so it can take a click and still render as
  // nothing. Assert a real box, not just a hit.
  await expect(tile).toContainText("Alpha");
  expect((await tile.boundingBox())?.width ?? 0).toBeGreaterThan(40);
  await tile.click();
  // The card reads as picked, and the submit button — the one place the count
  // is reported — says what it will do.
  await expect(tile).toHaveAttribute("aria-pressed", "true");
  await expect(modal.getByTestId("add-books-submit")).toHaveText("Add 1 book");
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/add-books",
      expectedBody: { id, book_uuids: [alpha] },
      expectedStatus: 200,
    },
    async () => modal.getByTestId("add-books-submit").click(),
  );

  await expect(modal).toHaveCount(0);
  await expect(grid(page)).toContainText("Alpha");
});

test("keeps the picker open and says so when adding fails", async ({
  page,
  request,
}) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Add Fail ${Date.now()}`,
  });

  await openShelfFromIndex(page, id);
  await page.route("**/api/rpc/shelves/add-books", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );
  await addButton(page).click();
  const modal = page.getByTestId("add-books-modal");
  await modal.getByTestId("add-books-search").fill("Alpha");
  await modal.getByTestId(`picker-tile-${alpha}`).click();
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/add-books", expectedStatus: 500 },
    async () => modal.getByTestId("add-books-submit").click(),
  );

  await expect(modal.getByTestId("add-books-error")).toBeVisible();
  await expect(modal).toBeVisible();
});

test("marks a book already on the shelf instead of offering it again", async ({
  page,
  request,
}) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Member ${Date.now()}`,
    book_uuids: [alpha],
  });

  await openShelfFromIndex(page, id);
  await addButton(page).click();
  const modal = page.getByTestId("add-books-modal");
  await modal.getByTestId("add-books-search").fill("Alpha");
  const tile = modal.getByTestId(`picker-tile-${alpha}`);

  await expect(tile).toContainText("On this shelf");
  await expect(tile).toBeDisabled();
  // Inert, not merely styled: a member can't be picked into a second copy.
  await tile.click({ force: true });
  await expect(modal.getByTestId("add-books-submit")).toHaveText("Add books");
  await expect(modal.getByTestId("add-books-submit")).toBeDisabled();
});

test("takes a book off a hand-picked shelf", async ({ page, request }) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Remove ${Date.now()}`,
    book_uuids: [alpha],
  });

  await openShelfFromIndex(page, id);
  await expect(grid(page)).toContainText("Alpha");
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/remove-book",
      expectedBody: { id, book_uuid: alpha },
      expectedStatus: 200,
    },
    async () =>
      page
        .getByRole("button", { name: "Remove Alpha from this shelf" })
        .click(),
  );

  await expect(page.getByTestId("shelf-empty")).toBeVisible();
});

test("keeps the book and says so when taking it off fails", async ({
  page,
  request,
}) => {
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Remove Fail ${Date.now()}`,
    book_uuids: [alpha],
  });

  await openShelfFromIndex(page, id);
  await page.route("**/api/rpc/shelves/remove-book", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/remove-book",
      expectedStatus: 500,
    },
    async () =>
      page
        .getByRole("button", { name: "Remove Alpha from this shelf" })
        .click(),
  );

  await expect(page.getByTestId("shelf-remove-error")).toContainText(
    "Couldn’t remove “Alpha”",
  );
  await expect(grid(page)).toContainText("Alpha");
});

test("renames a shelf from Edit shelf", async ({ page, request }) => {
  const name = `E2E Detail Rename ${Date.now()}`;
  const id = await createShelf(request, { kind: "manual", name });

  await openShelfFromIndex(page, id);
  await editButton(page).click();
  const modal = page.getByTestId("edit-shelf-modal");
  const renamed = `${name} (renamed)`;
  await modal.getByTestId("edit-shelf-name").fill(renamed);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/update", expectedStatus: 200 },
    async () => modal.getByTestId("edit-shelf-save").click(),
  );

  await expect(modal).toHaveCount(0);
  await expect(title(page, renamed)).toBeVisible();
});

test("keeps the name and says so when a rename fails", async ({
  page,
  request,
}) => {
  const name = `E2E Detail Rename Fail ${Date.now()}`;
  const id = await createShelf(request, { kind: "manual", name });

  await openShelfFromIndex(page, id);
  await page.route("**/api/rpc/shelves/update", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );
  await editButton(page).click();
  const modal = page.getByTestId("edit-shelf-modal");
  await modal.getByTestId("edit-shelf-name").fill(`${name} v2`);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/update", expectedStatus: 500 },
    async () => modal.getByTestId("edit-shelf-save").click(),
  );

  await expect(modal.getByTestId("edit-shelf-error")).toBeVisible();
  await expect(title(page, name)).toBeVisible();
});

test("turns on Kobo sync from the More menu", async ({ page, request }) => {
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Kobo ${Date.now()}`,
  });

  await openShelfFromIndex(page, id);
  await moreButton(page).click();
  const toggle = page.getByRole("menuitemcheckbox", { name: "Sync to Kobo" });
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/update",
      expectedBody: {
        id,
        req: {
          name: null,
          description: null,
          visibility: null,
          match_mode: null,
          rules: null,
          sync_to_kobo: true,
        },
      },
      expectedStatus: 200,
    },
    async () => toggle.click(),
  );

  await expect(page.getByTestId("shelf-kobo-badge")).toBeVisible();
});

test("deletes a shelf and returns to the index", async ({ page, request }) => {
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Detail Delete ${Date.now()}`,
  });

  await openShelfFromIndex(page, id);
  await moreButton(page).click();
  await page.getByTestId("shelf-delete").click();
  await expect(page.getByTestId("shelf-delete-modal")).toBeVisible();
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/delete",
      expectedBody: { id },
      expectedStatus: 200,
    },
    async () => page.getByTestId("shelf-delete-confirm").click(),
  );

  await expect(page).toHaveURL(/\/shelves$/);
  await expect(page.getByTestId(`shelf-card-${id}`)).toHaveCount(0);
});

test("keeps the shelf and says so when deleting fails", async ({
  page,
  request,
}) => {
  const name = `E2E Detail Delete Fail ${Date.now()}`;
  const id = await createShelf(request, { kind: "manual", name });

  await openShelfFromIndex(page, id);
  await page.route("**/api/rpc/shelves/delete", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );
  await moreButton(page).click();
  await page.getByTestId("shelf-delete").click();
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/delete", expectedStatus: 500 },
    async () => page.getByTestId("shelf-delete-confirm").click(),
  );

  await expect(page.getByTestId("shelf-delete-error")).toBeVisible();
  await expect(page.getByTestId("shelf-delete-confirm")).toBeEnabled();
  await expect(page).toHaveURL(new RegExp(`/shelves/${id}$`));
});

test("shows a smart shelf's rules and re-sorts its books", async ({
  page,
  request,
}) => {
  // "Ada Lovelace" matches the Alpha fixtures — read, never written.
  const id = await createShelf(request, {
    kind: "smart",
    name: `E2E Detail Smart ${Date.now()}`,
    match_mode: "any",
    rules: [{ field: "author", op: "is", value: "Ada Lovelace" }],
  });

  await openShelfFromIndex(page, id);
  await expect(page.getByTestId("shelf-rules")).toContainText(
    "Author is Ada Lovelace",
  );
  // Rules fill a smart shelf, so there is nothing to add by hand.
  await expect(addButton(page)).toHaveCount(0);
  await expect(grid(page)).toContainText("Alpha");

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.getByTestId("shelf-sort").selectOption("title"),
  );
  await expect(grid(page)).toContainText("Alpha");
});

test("greys out every change on a Wishlist and says why", async ({
  page,
  request,
}) => {
  const id = await ownWishlistId(request);

  await openShelfFromIndex(page, id);

  await expect(editButton(page)).toBeDisabled();
  await expect(moreButton(page)).toBeDisabled();
  await expect(addButton(page)).toHaveCount(0);
  await expect(lockNote(page)).toContainText(
    "Your wishlist can’t be edited here.",
  );
  await expect(editButton(page)).toHaveAttribute(
    "aria-describedby",
    "shd-lock-reason",
  );
  // A greyed control is inert, not merely styled.
  await editButton(page).click({ force: true });
  await expect(page.getByTestId("edit-shelf-modal")).toHaveCount(0);
});

test("greys out another reader's shelf and names its owner", async ({
  browser,
  request,
}) => {
  const name = `E2E Detail Locked ${Date.now()}`;
  const id = await createShelf(request, {
    kind: "manual",
    name,
    visibility: "public",
  });
  const username = `e2e_shelf_viewer_${Date.now()}`;
  const readerId = await createReader(request, username);
  const context = await browser.newContext({
    storageState: { cookies: [], origins: [] },
  });
  const page = await context.newPage();
  try {
    await logIn(page, username);
    await openShelfFromIndex(page, id);

    await expect(title(page, name)).toBeVisible();
    await expect(addButton(page)).toBeDisabled();
    await expect(editButton(page)).toBeDisabled();
    await expect(moreButton(page)).toBeDisabled();
    const owner = (
      await page.getByTestId("shelf-owner-name").textContent()
    )?.trim();
    expect(owner, "the shelf names its owner").toBeTruthy();
    await expect(lockNote(page)).toContainText(
      `You can’t edit this shelf — it belongs to ${owner}.`,
    );

    await addButton(page).click({ force: true });
    await expect(page.getByTestId("add-books-modal")).toHaveCount(0);
  } finally {
    await context.close();
    await request.delete(`/api/users/${readerId}`);
  }
});

test("lets an admin change another reader's shelf without explaining why", async ({
  page,
  request,
  playwright,
  baseURL,
}) => {
  const username = `e2e_shelf_owner_${Date.now()}`;
  const readerId = await createReader(request, username);
  try {
    const id = await createShelfAs(
      playwright.request,
      baseURL ?? "",
      username,
      {
        kind: "manual",
        name: `E2E Reader Shelf ${Date.now()}`,
      },
    );

    await openShelfFromIndex(page, id);

    // The hero still names the owner; nothing explains why an admin *may*
    // edit, because only a refusal earns a note.
    await expect(page.getByTestId("shelf-owner-name")).toHaveText(username);
    await expect(page.getByTestId("shelf-admin-note")).toHaveCount(0);
    await expect(editButton(page)).toBeEnabled();
    await expect(lockNote(page)).toHaveCount(0);
  } finally {
    await request.delete(`/api/users/${readerId}`);
  }
});
