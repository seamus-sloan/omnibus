import type { APIRequestContext } from "@playwright/test";

import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The gallery filters the landing book list in place, so it needs the real
// fixture library plus a manual shelf holding one distinct book. Alpha is
// also used by shelves.spec.ts rail tests, but only ever read — no spec
// mutates it, per the fixture-ownership rule.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const createManualShelf = async (
  request: APIRequestContext,
  name: string,
  bookUuids: string[],
) => {
  const resp = await request.post("/api/rpc/shelves/create", {
    data: {
      req: {
        kind: "manual",
        name,
        description: null,
        visibility: "private",
        match_mode: null,
        rules: [],
        book_uuids: bookUuids,
      },
    },
  });
  expect(resp.status(), `POST /api/rpc/shelves/create failed for ${name}`).toBe(
    200,
  );
  const shelf = (await resp.json()) as { id: number };
  return shelf.id;
};

test("selects a shelf in place: glow moves, list filters, URL stays put", async ({
  page,
  request,
}) => {
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Gallery ${Date.now()}`;
  const shelfId = await createManualShelf(request, name, [alphaUuid]);

  await gotoReady(page, "/");

  // All Books glows (is selected) by default.
  await expect(page.getByTestId("gallery-all-books")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  const tile = page.getByTestId(`gallery-shelf-${shelfId}`);
  await expect(tile).toBeVisible();
  // The member fetch is a server-fn POST, so await it like a mutation to
  // guarantee the list swap has landed before asserting UI state.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => tile.click(),
  );

  await expect(tile).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("gallery-all-books")).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  await expect(page.getByTestId("lib-section-title")).toContainText(name);
  // Only the shelf member renders; selection filtered in place (no navigation).
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);
  await expect(page.getByTestId("ebook-table")).toContainText("Alpha");
  expect(new URL(page.url()).pathname).toBe("/");
});

test("persists the selected shelf across a reload", async ({
  page,
  request,
}) => {
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Gallery Persist ${Date.now()}`;
  const shelfId = await createManualShelf(request, name, [alphaUuid]);

  await gotoReady(page, "/");
  const tile = page.getByTestId(`gallery-shelf-${shelfId}`);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => tile.click(),
  );
  await expect(tile).toHaveAttribute("aria-pressed", "true");

  // The persisted pick reconciles post-mount (rule 07) and refires the
  // member fetch — await it so the assertions can't race the list swap.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.reload(),
  );

  await expect(page.getByTestId(`gallery-shelf-${shelfId}`)).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("lib-section-title")).toContainText(name);
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);
});

test("returns to All Books and restores the full library", async ({
  page,
  request,
}) => {
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Gallery Back ${Date.now()}`;
  const shelfId = await createManualShelf(request, name, [alphaUuid]);

  await gotoReady(page, "/");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.getByTestId(`gallery-shelf-${shelfId}`).click(),
  );
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);

  await page.getByTestId("gallery-all-books").click();

  await expect(page.getByTestId("gallery-all-books")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("lib-section-title")).toContainText(
    "All Books",
  );
  await expect
    .poll(async () => page.getByTestId(/^ebook-row-/).count())
    .toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);
});

test("edits the selected shelf from the landing header pencil", async ({
  page,
  request,
}) => {
  // A smart shelf so the facet row has a rule chip to assert. The author
  // rule only reads fixture books — nothing here mutates a shared fixture.
  const name = `E2E Gallery Edit ${Date.now()}`;
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

  await gotoReady(page, "/");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.getByTestId(`gallery-shelf-${shelf.id}`).click(),
  );

  // Facet row under the section title states the kind, visibility, and the
  // rule query once the full-shelf fetch lands.
  const facets = page.getByTestId("shelf-facets");
  await expect(facets).toContainText("Smart shelf");
  await expect(facets).toContainText("Private");
  await expect(facets).toContainText("Author is Ada Lovelace");

  await page.getByTestId("shelf-edit").click();
  const modal = page.getByTestId("edit-shelf-modal");
  await expect(modal.getByTestId("edit-shelf-name")).toHaveValue(name);

  const renamed = `${name} v2`;
  await modal.getByTestId("edit-shelf-name").fill(renamed);
  await modal.getByTestId("shelf-vis-public").click();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/update",
      expectedBody: {
        id: shelf.id,
        req: {
          name: renamed,
          description: null,
          visibility: "public",
          match_mode: "any",
          rules: [{ field: "author", op: "is", value: "Ada Lovelace" }],
          sync_to_kobo: false,
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

test("shows the continue-reading hero once a book has progress", async ({
  page,
  request,
}) => {
  // Seed an epub position for a fixture book via the REST progress endpoint.
  // Progress is per-(user, book); Alpha's position is only ever written here
  // and never asserted by the reader progress-restore specs (which own
  // frankenstein / great-gatsby exclusively).
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const resp = await request.post("/api/progress", {
    data: {
      book_uuid: alphaUuid,
      format: "epub",
      epub_cfi: "epubcfi(/6/4!/4/2/2[c01]/2/1:0)",
    },
  });
  expect(resp.status(), "POST /api/progress failed").toBe(200);

  await gotoReady(page, "/");

  await expect(page.getByTestId("continue-hero")).toBeVisible();
  await expect(page.getByTestId(`hero-card-${alphaUuid}`)).toBeVisible();
  // The epub CTA deep-links into the reader.
  await expect(page.getByTestId(`hero-resume-${alphaUuid}`)).toHaveAttribute(
    "href",
    `/read/${alphaUuid}`,
  );
});
