// Genres: the metadata-edit chip section, the landing table's Genres cell,
// and the book-detail chips.
//
// Genres are unlike every other editable field here — nothing the scanner
// parses carries one, so a book has genres only because someone assigned
// them, and they live solely in `metadata_overrides`. Every assertion below
// therefore writes shared server state, which is why this file owns
// `standalone-tundra` outright (see the fixture comment in
// `tests/fixtures/epubs.ts`).

import type { APIRequestContext } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle, switchToTableView } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "standalone-tundra")!;

/** The save endpoint every genre write goes through. */
const OVERRIDES_RPC = /\/api\/rpc\/ebook\/overrides$/;

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

/** Drop every override on the target so each test starts genre-free. */
async function clearOverrides(request: APIRequestContext) {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  const resp = await request.post("/api/rpc/ebook/overrides/delete", {
    data: { uuid },
  });
  expect(resp.status(), "cleanup revert must succeed").toBe(200);
}

test.beforeEach(async ({ request }) => {
  await clearOverrides(request);
});

test.afterAll(async ({ request }) => {
  await clearOverrides(request);
});

test("renders the metadata edit page's Genres section", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}/edit`);
  await expectNavVisible(page);

  const section = page.getByTestId("me-genres-section");
  await expect(section).toBeVisible();
  // The label distinguishes it from the Tags section directly above, which
  // is the same control over a different vocabulary.
  await expect(page.getByText("Genres", { exact: true })).toBeVisible();
  await expect(section.getByTestId("me-genres-input")).toHaveAttribute(
    "placeholder",
    "+ add genre…",
  );
});

test("assigns genres from the metadata edit page and shows them on book detail", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}/edit`);

  const section = page.getByTestId("me-genres-section");
  const input = section.getByTestId("me-genres-input");

  // Wait for each chip to land before typing the next: Enter clears the
  // input and re-renders the chip list, so a second `fill` issued before
  // that settles races the re-render and is dropped.
  await input.fill("Hard Science Fiction");
  await input.press("Enter");
  await expect(section).toContainText("Hard Science Fiction");
  await input.fill("Adventure");
  await input.press("Enter");
  await expect(section).toContainText("Adventure");

  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 200 },
    async () => page.getByRole("button", { name: /Save/ }).click(),
  );

  // Saving navigates back to the detail page, where genres render as their
  // own chip list above the tag list.
  await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
  const genreList = page.getByTestId("bd-genre-list");
  await expect(genreList).toBeVisible();
  await expect(genreList).toContainText("Hard Science Fiction");
  await expect(genreList).toContainText("Adventure");
});

test("editing genres leaves the scanned tag list untouched", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  await gotoReady(page, `/books/${uuid}/edit`);
  const section = page.getByTestId("me-genres-section");
  const input = section.getByTestId("me-genres-input");
  await input.fill("Noir");
  await input.press("Enter");
  await expect(section).toContainText("Noir");
  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 200 },
    async () => page.getByRole("button", { name: /Save/ }).click(),
  );

  // A genres override must not write a `subjects` one: doing so would pin
  // the scanned tag list against a later rescan.
  const resp = await request.get(`/api/ebooks/${uuid}`);
  expect(resp.status()).toBe(200);
  const book = (await resp.json()) as {
    genres?: string[];
    subjects: string[];
  };
  expect(book.genres).toEqual(["Noir"]);
  expect(book.subjects).toEqual(TARGET.tags ?? []);
});

test("clicking the Genres cell in the table renders the chip editor inline", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const cell = page
    .getByTestId(`ebook-row-${TARGET.slug}`)
    .getByTestId("ebook-cell-genres");
  await cell.click();

  const chipInput = cell.getByTestId("ebook-cell-genres-input");
  await expect(chipInput).toBeVisible();
  await expect(chipInput).toHaveAttribute("placeholder", "+ add genre…");
  await expect(cell).toHaveClass(/ebook-cell-editing/);

  // Escape exits edit mode without saving.
  await chipInput.press("Escape");
  await expect(chipInput).toHaveCount(0);
  await expect(cell).not.toHaveClass(/ebook-cell-editing/);
});

test("assigns a genre inline from the table and reflects it in the cell", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const cell = page
    .getByTestId(`ebook-row-${TARGET.slug}`)
    .getByTestId("ebook-cell-genres");
  await cell.click();

  const chipInput = cell.getByTestId("ebook-cell-genres-input");
  await chipInput.fill("Techno-Thriller");
  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 200 },
    async () => chipInput.press("Enter"),
  );

  // Stayed on the landing page; the cell shows the server-merged value.
  await expect(page).toHaveURL(/\/$/);
  await expect(cell).toContainText("Techno-Thriller");
  await chipInput.press("Escape");
  await expect(cell).toContainText("Techno-Thriller");
});

test("surfaces a failed genre save by leaving the cell unchanged", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const cell = page
    .getByTestId(`ebook-row-${TARGET.slug}`)
    .getByTestId("ebook-cell-genres");
  await cell.click();
  const chipInput = cell.getByTestId("ebook-cell-genres-input");

  await page.route(OVERRIDES_RPC, (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "boom" }),
    }),
  );

  await chipInput.fill("Never Saved");
  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 500 },
    async () => chipInput.press("Enter"),
  );

  // The optimistic book state is only replaced on a successful response, so
  // a rejected save must not leave the genre in the committed cell text.
  await page.unroute(OVERRIDES_RPC);
  await gotoReady(page, "/");
  await switchToTableView(page);
  const reloaded = page
    .getByTestId(`ebook-row-${TARGET.slug}`)
    .getByTestId("ebook-cell-genres");
  await expect(reloaded).not.toContainText("Never Saved");
});
