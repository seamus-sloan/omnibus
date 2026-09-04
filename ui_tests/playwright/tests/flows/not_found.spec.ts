import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";

// An unmatched URL used to render dioxus-router's own parse diagnostic as the
// page body — the complete internal route table, admin paths included, with no
// nav and no link back (#2214). The catch-all `Route::NotFound` replaces it.

test("renders the not-found layout for an unmatched path", async ({ page }) => {
  await gotoReady(page, "/add");

  await expectNavVisible(page);
  const body = page.getByTestId("not-found-page");
  await expect(body).toBeVisible();
  await expect(
    body.getByRole("heading", { name: "Page not found" }),
  ).toBeVisible();
  await expect(body).toContainText("/add");
});

// `/tags` (the tag cloud, #2157) was a real route long enough for a reader to
// have bookmarked it. Probed by name rather than left to the `/add` case above:
// only a retired route catches a later `#[route("/tags")]` re-declared above the
// catch-all, which an arbitrary unmatched path would still pass through.
test("a retired route falls through to not-found", async ({ page }) => {
  await gotoReady(page, "/tags");

  const body = page.getByTestId("not-found-page");
  await expect(body).toBeVisible();
  await expect(body).toContainText("/tags");
});

test("never publishes the internal route table to a mistyped URL", async ({
  page,
}) => {
  await gotoReady(page, "/nope/deeper/still");

  const body = await page.locator("body").innerText();
  expect(body).not.toContain("Attempted Matches");
  expect(body).not.toContain("Failed to parse route");
  expect(body).not.toContain("admin/health");
});

test("returns to the library from the not-found page", async ({ page }) => {
  await gotoReady(page, "/add");

  await page
    .getByTestId("not-found-page")
    .getByRole("link", { name: "Back to library" })
    .click();

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("not-found-page")).toHaveCount(0);
});
