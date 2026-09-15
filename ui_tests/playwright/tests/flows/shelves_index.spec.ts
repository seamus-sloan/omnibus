import type { APIRequestContext } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import { createShelf, openShelfFromIndex, shelfCard } from "../utils/shelves";

// The `/shelves` index lists every shelf the viewer can see, grouped by owner.
// The suite runs as an admin, who sees every shelf every parallel spec has
// made — so each test asserts on its own shelves by id, and narrows the list
// with a per-run stamp before asserting on a count.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const filterInput = (page: import("@playwright/test").Page) =>
  page.getByLabel("Filter shelves by name or owner");

/**
 * Create a non-admin reader through the admin API and return their id. The
 * account comes with its public Wishlist shelf, which is what puts a second
 * owner on the index without signing in as them.
 */
async function createReader(
  request: APIRequestContext,
  username: string,
): Promise<number> {
  const resp = await request.post("/api/users", {
    data: {
      username,
      password: "shelf-owner-pw-0001",
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

test("renders the shelves index layout", async ({ page }) => {
  await gotoReady(page, "/shelves");

  await expectNavVisible(page);
  await expect(
    page.getByRole("heading", { level: 1, name: "Shelves" }),
  ).toBeVisible();
  await expect(page.getByTestId("shelves-census")).toBeVisible();
  await expect(page.getByTestId("new-shelf")).toBeVisible();
  await expect(filterInput(page)).toBeVisible();
  // The viewer's own shelves lead the page.
  await expect(
    page
      .getByTestId(/^shelves-group-\d+$/)
      .first()
      .getByRole("heading"),
  ).toHaveText("Your shelves");
});

test("reaches the index from the library's shelves row", async ({ page }) => {
  // The library's shelf section is the only way in — the top nav has no
  // Shelves link.
  await gotoReady(page, "/");
  await expect(
    page
      .getByRole("navigation", { name: "Primary" })
      .getByRole("link", { name: "Shelves" }),
  ).toHaveCount(0);

  await page.getByTestId("gallery-all-shelves").click();
  await expect(page).toHaveURL(/\/shelves$/);
  await expect(page.getByTestId("shelves-index")).toBeVisible();
});

test("opens a shelf from its card", async ({ page, request }) => {
  const name = `E2E Index Open ${Date.now()}`;
  const id = await createShelf(request, { kind: "manual", name });

  await openShelfFromIndex(page, id);

  await expect(page.getByRole("heading", { level: 1, name })).toBeVisible();
});

test("filters by name and offers a way back when nothing matches", async ({
  page,
  request,
}) => {
  const stamp = Date.now();
  const findable = await createShelf(request, {
    kind: "manual",
    name: `E2E Findable ${stamp}`,
  });
  const other = await createShelf(request, {
    kind: "manual",
    name: `E2E Elsewhere ${stamp}`,
  });

  await gotoReady(page, "/shelves");
  // Terms match independently and case-insensitively.
  await filterInput(page).fill(`findable ${stamp}`);
  await expect(shelfCard(page, findable)).toBeVisible();
  await expect(shelfCard(page, other)).toHaveCount(0);
  await expect(page.getByTestId(/^shelf-card-\d+$/)).toHaveCount(1);

  await filterInput(page).fill(`nothing is called ${stamp}`);
  await expect(page.getByTestId("shelves-empty")).toContainText(
    "No shelves match",
  );
  await page.getByTestId("shelves-clear-filters").click();
  await expect(filterInput(page)).toHaveValue("");
  await expect(shelfCard(page, findable)).toBeVisible();
});

test("narrows the index to one kind of shelf", async ({ page, request }) => {
  const id = await createShelf(request, {
    kind: "manual",
    name: `E2E Kind ${Date.now()}`,
  });

  await gotoReady(page, "/shelves");
  await expect(shelfCard(page, id)).toBeVisible();

  await page.getByTestId("shelves-kind-wishlist").click();
  await expect(page.getByTestId("shelves-kind-wishlist")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(shelfCard(page, id)).toHaveCount(0);
  // Every account has a Wishlist, so the filter never empties the page.
  await expect(page.getByTestId(/^shelf-card-\d+$/).first()).toContainText(
    "Wishlist",
  );

  await page.getByTestId("shelves-kind-manual").click();
  await expect(shelfCard(page, id)).toBeVisible();
});

test("sorts by name or by size and remembers the choice", async ({
  page,
  request,
}) => {
  // Read-only fixture books: nothing here writes to them.
  const alpha = await fetchBookUuidByTitle(request, "Alpha");
  const beta = await fetchBookUuidByTitle(request, "Beta in the Series");
  const stamp = Date.now();
  const small = await createShelf(request, {
    kind: "manual",
    name: `E2E Sort A ${stamp}`,
    book_uuids: [alpha],
  });
  const large = await createShelf(request, {
    kind: "manual",
    name: `E2E Sort B ${stamp}`,
    book_uuids: [alpha, beta],
  });

  await gotoReady(page, "/shelves");
  await filterInput(page).fill(`${stamp}`);
  const cards = page.getByTestId(/^shelf-card-\d+$/);
  await expect(cards).toHaveCount(2);
  await expect(cards.nth(0)).toHaveAttribute(
    "data-testid",
    `shelf-card-${small}`,
  );

  await page.getByTestId("shelves-sort-count").click();
  await expect(cards.nth(0)).toHaveAttribute(
    "data-testid",
    `shelf-card-${large}`,
  );
  await expect(shelfCard(page, large)).toContainText("2 books");

  await page.reload();
  await page.waitForLoadState("networkidle");
  await expect(page.getByTestId("shelves-sort-count")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

test("groups another reader's shelves under their name and filters to them", async ({
  page,
  request,
}) => {
  const username = `e2e_shelf_owner_${Date.now()}`;
  const readerId = await createReader(request, username);
  try {
    await gotoReady(page, "/shelves");
    const group = page.getByTestId(`shelves-group-${readerId}`);
    await expect(
      group.getByRole("heading", { name: `${username}’s shelves` }),
    ).toBeVisible();
    await expect(group).toContainText(`${username}'s Wishlist`);

    const chip = page.getByTestId(`shelves-owner-${readerId}`);
    await chip.click();
    await expect(chip).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId(/^shelves-group-\d+$/)).toHaveCount(1);
    await expect(group).toBeVisible();

    // Picking the same owner again widens back to everyone.
    await chip.click();
    await expect(page.getByTestId("shelves-owner-anyone")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect
      .poll(async () => page.getByTestId(/^shelves-group-\d+$/).count())
      .toBeGreaterThan(1);
  } finally {
    await request.delete(`/api/users/${readerId}`);
  }
});

test("creates a shelf from the index and lands on it", async ({ page }) => {
  await gotoReady(page, "/shelves");

  await page.getByTestId("new-shelf").click();
  const modal = page.getByTestId("create-shelf-modal");
  await modal.getByRole("button", { name: /hand-picked/i }).click();
  const name = `E2E Index Create ${Date.now()}`;
  await modal.getByTestId("shelf-name-input").fill(name);

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/create", expectedStatus: 200 },
    async () => modal.getByTestId("shelf-create-submit").click(),
  );

  await expect(page).toHaveURL(/\/shelves\/\d+$/);
  await expect(page.getByRole("heading", { level: 1, name })).toBeVisible();
});

test("surfaces an error when the shelf list fails to load", async ({
  page,
}) => {
  await page.route("**/api/rpc/shelves", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await page.goto("/shelves");

  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByTestId("shelves-index")).toHaveCount(0);
});
