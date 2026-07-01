import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { expectMutation } from "../utils/api";
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

test("creates a hand-picked shelf and shows it in the rail", async ({ page }) => {
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
