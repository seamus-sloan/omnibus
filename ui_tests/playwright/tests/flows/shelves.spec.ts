import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Shelves need a real library so the create modal's hand-picked picker and the
// rail have books to work with. The settings POST kicks off an async reindex,
// so `seedLibrary` polls until every fixture EPUB is surfaced.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test("renders the shelves rail on the library page", async ({ page }) => {
  await gotoReady(page, "/");

  // The rail replaces the old filter row: "All books" + a New-shelf button.
  await expect(page.getByTestId("rail-all-books")).toBeVisible();
  await expect(page.getByTestId("new-shelf")).toBeVisible();
  await expectNavVisible(page);
});

test("creates a hand-picked shelf and shows it in the rail", async ({
  page,
}) => {
  await gotoReady(page, "/");

  await page.getByTestId("new-shelf").click();
  const modal = page.getByTestId("create-shelf-modal");
  await expect(modal).toBeVisible();

  // Switch to the hand-picked kind and name it. A hand-picked shelf with no
  // books yet is valid, so this exercises the create contract without the
  // (unit-tested) smart rule builder.
  await modal.getByRole("button", { name: /hand-picked/i }).click();
  const name = `E2E Picks ${Date.now()}`;
  await modal.getByTestId("shelf-name-input").fill(name);

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/create", expectedStatus: 200 },
    async () => modal.getByTestId("shelf-create-submit").click(),
  );

  // The rail refetches and the new shelf appears.
  await expect(page.getByText(name)).toBeVisible();
});

test("surfaces an error when shelf creation fails", async ({ page }) => {
  await gotoReady(page, "/");

  // Force the create server function to 500.
  await page.route("**/api/rpc/shelves/create", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await page.getByTestId("new-shelf").click();
  const modal = page.getByTestId("create-shelf-modal");
  await modal.getByRole("button", { name: /hand-picked/i }).click();
  await modal.getByTestId("shelf-name-input").fill("Doomed shelf");

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/create", expectedStatus: 500 },
    async () => modal.getByTestId("shelf-create-submit").click(),
  );

  await expect(page.getByTestId("shelf-create-error")).toBeVisible();
});

test("surfaces a distinct error when the member-books refetch fails", async ({
  page,
  request,
}) => {
  // A genuinely empty manual shelf renders the grid with zero tiles and no
  // error text — force the page-fetch to 500 so this test can tell the two
  // states apart via the dedicated `shelf-refetch-error` banner.
  const name = `E2E Refetch Fail ${Date.now()}`;
  const createResp = await request.post("/api/rpc/shelves/create", {
    data: {
      req: {
        kind: "manual",
        name,
        description: null,
        visibility: "private",
        match_mode: null,
        rules: [],
        book_uuids: [],
      },
    },
  });
  expect(createResp.status(), "POST /api/rpc/shelves/create failed").toBe(200);
  const shelf = (await createResp.json()) as { id: number };

  await page.route("**/api/rpc/shelves/page", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 500 },
    async () => gotoReady(page, `/shelves/${shelf.id}`),
  );

  await expect(page.getByTestId("shelf-detail-header")).toContainText(name);
  await expect(page.getByTestId("shelf-refetch-error")).toBeVisible();
});

test("switches shelves via the rail without a full reload", async ({
  page,
  request,
}) => {
  // Two hand-picked shelves, each holding one distinct fixture book, so the
  // header and grid content can only match if the rail switch actually
  // re-fetched.
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const betaUuid = await fetchBookUuidByTitle(request, "Beta in the Series");

  const nameA = `E2E Rail A ${Date.now()}`;
  const nameB = `E2E Rail B ${Date.now()}`;

  const createManualShelf = async (name: string, bookUuid: string) => {
    const resp = await request.post("/api/rpc/shelves/create", {
      data: {
        req: {
          kind: "manual",
          name,
          description: null,
          visibility: "private",
          match_mode: null,
          rules: [],
          book_uuids: [bookUuid],
        },
      },
    });
    expect(
      resp.status(),
      `POST /api/rpc/shelves/create failed for ${name}`,
    ).toBe(200);
    const shelf = (await resp.json()) as { id: number };
    return shelf.id;
  };

  const shelfAId = await createManualShelf(nameA, alphaUuid);
  const shelfBId = await createManualShelf(nameB, betaUuid);

  await gotoReady(page, `/shelves/${shelfAId}`);
  await expect(page.getByTestId("shelf-detail-header")).toContainText(nameA);
  await expect(page.getByTestId("shelf-grid")).toContainText("Alpha");

  await page.getByTestId(`rail-shelf-${shelfBId}`).click();

  await expect(page.getByTestId("shelf-detail-header")).toContainText(nameB);
  await expect(page.getByTestId("shelf-grid")).toContainText(
    "Beta in the Series",
  );
  await expect(page.getByTestId("shelf-grid")).not.toContainText("Alpha");

  // And back the other way, confirming both directions re-fetch.
  await page.getByTestId(`rail-shelf-${shelfAId}`).click();

  await expect(page.getByTestId("shelf-detail-header")).toContainText(nameA);
  await expect(page.getByTestId("shelf-grid")).toContainText("Alpha");
  await expect(page.getByTestId("shelf-grid")).not.toContainText(
    "Beta in the Series",
  );
});

test("edits an existing smart shelf's rules and the member grid updates", async ({
  page,
  request,
}) => {
  // "Ada Lovelace" matches the Alpha ebook + its audiobook; "Grace Hopper"
  // matches the Beta ebook + its audiobook — disjoint two-book matches, so a
  // membership change after save can only be explained by the edited rule
  // actually taking effect.
  const name = `E2E Smart Edit ${Date.now()}`;
  const createResp = await request.post("/api/rpc/shelves/create", {
    data: {
      req: {
        kind: "smart",
        name,
        description: null,
        visibility: "private",
        match_mode: "any",
        rules: [{ field: "author", op: "is", value: "Ada Lovelace" }],
        book_uuids: [],
      },
    },
  });
  expect(createResp.status(), "POST /api/rpc/shelves/create failed").toBe(200);
  const shelf = (await createResp.json()) as { id: number };

  await gotoReady(page, `/shelves/${shelf.id}`);
  await expect(page.getByTestId("shelf-grid")).toContainText("Alpha");
  await expect(page.getByTestId("shelf-grid")).not.toContainText(
    "Beta in the Series",
  );

  await page.getByTestId("shelf-actions").click();
  await page.getByTestId("shelf-edit-rules").click();

  const modal = page.getByTestId("edit-shelf-rules-modal");
  await expect(modal).toBeVisible();

  // Prefilled from the shelf's current rule.
  const valueInput = modal.getByTestId("condition-row-0").locator("input");
  await expect(valueInput).toHaveValue("Ada Lovelace");
  await expect(modal.getByTestId("rule-preview-count")).toContainText("2 of");

  await valueInput.fill("Grace Hopper");
  await expect(modal.getByTestId("rule-preview-count")).toContainText("2 of");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/update",
      expectedBody: {
        id: shelf.id,
        req: {
          name: null,
          description: null,
          visibility: null,
          match_mode: "any",
          rules: [{ field: "author", op: "is", value: "Grace Hopper" }],
        },
      },
      expectedStatus: 200,
    },
    async () => modal.getByTestId("edit-rules-save").click(),
  );

  await expect(modal).not.toBeVisible();
  await expect(page.getByTestId("shelf-grid")).toContainText(
    "Beta in the Series",
  );
  await expect(page.getByTestId("shelf-grid")).not.toContainText("Alpha");
});
