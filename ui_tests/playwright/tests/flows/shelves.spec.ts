import type { APIRequestContext } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import { galleryTile, selectShelfInGallery } from "../utils/shelves";

// Shelves need a real library so the create modal's hand-picked picker and the
// gallery have books to work with. The settings POST kicks off an async
// reindex, so `seedLibrary` polls until every fixture EPUB is surfaced.
//
// Every shelf here is opened the way a user opens one: by picking its tile in
// the landing gallery, which filters the landing book list in place. The
// `/shelves/:id` page has no entry point in the web UI, so nothing in this
// file may deep-link it — see `tests/utils/shelves.ts`.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

/** Create a shelf straight through the API and return its id. */
async function createShelf(
  request: APIRequestContext,
  body: Record<string, unknown>,
): Promise<number> {
  const resp = await request.post("/api/rpc/shelves/create", {
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
  expect(
    resp.status(),
    `POST /api/rpc/shelves/create failed for ${body.name}`,
  ).toBe(200);
  const shelf = (await resp.json()) as { id: number };
  return shelf.id;
}

test("renders the shelf gallery on the library page", async ({ page }) => {
  await gotoReady(page, "/");

  // The gallery replaces the old left rail: an "All Books" tile (selected by
  // default) + a New-shelf button in the gallery head.
  await expect(page.getByTestId("shelf-gallery")).toBeVisible();
  await expect(page.getByTestId("gallery-all-books")).toBeVisible();
  await expect(page.getByTestId("new-shelf")).toBeVisible();
  await expectNavVisible(page);
});

test("creates a hand-picked shelf and shows it in the gallery", async ({
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

  // The gallery refetches and the new shelf appears as a tile.
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
  // states apart via the header's `lib-page-error` banner.
  const name = `E2E Refetch Fail ${Date.now()}`;
  const shelfId = await createShelf(request, { kind: "manual", name });

  await page.route("**/api/rpc/shelves/page", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await gotoReady(page, "/");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 500 },
    async () => galleryTile(page, shelfId).click(),
  );

  await expect(page.getByTestId("lib-section-title")).toContainText(name);
  await expect(page.getByTestId("lib-page-error")).toBeVisible();
});

test("switches shelves via the gallery without leaving the library page", async ({
  page,
  request,
}) => {
  // Two hand-picked shelves, each holding one distinct fixture book, so the
  // header and grid content can only match if the gallery pick actually
  // re-fetched.
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const betaUuid = await fetchBookUuidByTitle(request, "Beta in the Series");

  const nameA = `E2E Rail A ${Date.now()}`;
  const nameB = `E2E Rail B ${Date.now()}`;
  const shelfAId = await createShelf(request, {
    kind: "manual",
    name: nameA,
    book_uuids: [alphaUuid],
  });
  const shelfBId = await createShelf(request, {
    kind: "manual",
    name: nameB,
    book_uuids: [betaUuid],
  });

  await gotoReady(page, "/");
  await selectShelfInGallery(page, shelfAId, nameA);
  await expect(page.getByTestId("lib-grid")).toContainText("Alpha");

  await selectShelfInGallery(page, shelfBId, nameB);
  await expect(page.getByTestId("lib-grid")).toContainText(
    "Beta in the Series",
  );
  await expect(page.getByTestId("lib-grid")).not.toContainText("Alpha");

  // And back the other way, confirming both directions re-fetch.
  await selectShelfInGallery(page, shelfAId, nameA);
  await expect(page.getByTestId("lib-grid")).toContainText("Alpha");
  await expect(page.getByTestId("lib-grid")).not.toContainText(
    "Beta in the Series",
  );

  // The gallery filters in place — a pick must never navigate away.
  expect(new URL(page.url()).pathname).toBe("/");
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
  const shelfId = await createShelf(request, {
    kind: "smart",
    name,
    match_mode: "any",
    rules: [{ field: "author", op: "is", value: "Ada Lovelace" }],
  });

  await gotoReady(page, "/");
  await selectShelfInGallery(page, shelfId, name);
  await expect(page.getByTestId("lib-grid")).toContainText("Alpha");
  await expect(page.getByTestId("lib-grid")).not.toContainText(
    "Beta in the Series",
  );

  // The facet row under the title states the kind and the rule query.
  const facets = page.getByTestId("shelf-facets");
  await expect(facets).toContainText("Smart shelf");
  await expect(facets).toContainText("Author is Ada Lovelace");

  await page.getByTestId("shelf-edit").click();

  const modal = page.getByTestId("edit-shelf-modal");
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
        id: shelfId,
        req: {
          name,
          description: null,
          visibility: "private",
          match_mode: "any",
          rules: [{ field: "author", op: "is", value: "Grace Hopper" }],
          // The modal always states the toggle's value for non-system shelves.
          sync_to_kobo: false,
        },
      },
      expectedStatus: 200,
    },
    async () => modal.getByTestId("edit-shelf-save").click(),
  );

  await expect(modal).not.toBeVisible();
  await expect(facets).toContainText("Author is Grace Hopper");
  await expect(page.getByTestId("lib-grid")).toContainText(
    "Beta in the Series",
  );
  await expect(page.getByTestId("lib-grid")).not.toContainText("Alpha");
});

test("edits a shelf's name, visibility, and Kobo sync from the pencil modal", async ({
  page,
  request,
}) => {
  const name = `E2E Edit Fields ${Date.now()}`;
  const shelfId = await createShelf(request, { kind: "manual", name });

  await gotoReady(page, "/");
  await selectShelfInGallery(page, shelfId, name);
  const facets = page.getByTestId("shelf-facets");
  await expect(facets).toContainText("Hand-picked shelf");
  await expect(facets).toContainText("Private");

  await page.getByTestId("shelf-edit").click();
  const modal = page.getByTestId("edit-shelf-modal");
  await expect(modal.getByTestId("edit-shelf-name")).toHaveValue(name);

  // The Kobo opt-in prefills from the shelf (off for a fresh shelf).
  await expect(modal.getByTestId("edit-shelf-kobo-off")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  const renamed = `${name} (renamed)`;
  await modal.getByTestId("edit-shelf-name").fill(renamed);
  await modal.getByTestId("shelf-vis-public").click();
  await modal.getByTestId("edit-shelf-kobo-on").click();

  // The saved Kobo opt-in is asserted on the wire here rather than in the UI:
  // the badge that renders it lives on the shelf-detail header, which the web
  // app never navigates to.
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/update",
      expectedBody: {
        id: shelfId,
        req: {
          name: renamed,
          description: null,
          visibility: "public",
          match_mode: null,
          rules: null,
          sync_to_kobo: true,
        },
      },
      expectedStatus: 200,
    },
    async () => modal.getByTestId("edit-shelf-save").click(),
  );

  await expect(modal).not.toBeVisible();
  await expect(page.getByTestId("lib-section-title")).toContainText(renamed);
  await expect(facets).toContainText("Public");
});

test("surfaces an error when the shelf edit save fails", async ({
  page,
  request,
}) => {
  const name = `E2E Edit Fail ${Date.now()}`;
  const shelfId = await createShelf(request, { kind: "manual", name });

  await gotoReady(page, "/");
  await selectShelfInGallery(page, shelfId, name);
  await page.route("**/api/rpc/shelves/update", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await page.getByTestId("shelf-edit").click();
  const modal = page.getByTestId("edit-shelf-modal");
  await modal.getByTestId("edit-shelf-name").fill(`${name} v2`);

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/update", expectedStatus: 500 },
    async () => modal.getByTestId("edit-shelf-save").click(),
  );

  await expect(modal.getByTestId("edit-shelf-error")).toBeVisible();
  // The header keeps the saved name — the failed edit must not leak in.
  await expect(page.getByTestId("lib-section-title")).toContainText(name);
});
