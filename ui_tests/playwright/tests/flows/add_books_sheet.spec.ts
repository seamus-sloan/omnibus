import { expect, test } from "../fixtures/test";
import { gotoReady } from "../utils/nav";

// The bottom tab bar (and its raised center action) only renders below the
// phone breakpoint, so this whole flow runs at a phone viewport. The bar is
// shared SSR markup — the native shell renders the same tree in a WebView —
// so verifying it here covers both web-phone and mobile.
test.describe("add-books sheet (phone viewport)", () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test("center action renders in the tab bar", async ({ page }) => {
    await gotoReady(page, "/");

    // Four tabs split around the raised center action; Series is no longer a
    // tab (it left the bar for the center action).
    const nav = page.getByRole("navigation", { name: "Primary" });
    await expect(nav.getByRole("link", { name: "Library" })).toBeVisible();
    await expect(page.getByTestId("tabbar-scan")).toBeVisible();
    await expect(page.getByTestId("tabbar-scan")).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  test("center action opens the sheet with Scan highlighted", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await expect(page.getByTestId("add-books-sheet")).toHaveCount(0);

    await page.getByTestId("tabbar-scan").click();

    // Sheet is up with all three chooser rows, Scan first and highlighted.
    await expect(page.getByTestId("add-books-sheet")).toBeVisible();
    await expect(page.getByTestId("tabbar-scan")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    const rows = page.getByTestId("add-books-row");
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText("Scan a barcode");
    await expect(rows.nth(0)).toHaveClass(/m-add-row--primary/);
    await expect(rows.nth(1)).toContainText("Upload a file");
    await expect(rows.nth(2)).toContainText("Enter an ISBN");
  });

  test("Scan opens the check-in overlay and closes the sheet", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await page.getByTestId("tabbar-scan").click();
    await page.getByTestId("add-books-row").nth(0).click();

    // Check-in is now a centered overlay, not a route: the flow floats in
    // place over the current page, no navigation.
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();
    await expect(page.getByTestId("check-in")).toBeVisible();
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByTestId("add-books-sheet")).toHaveCount(0);
  });

  test("Upload routes to the add-books page", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("tabbar-scan").click();
    await page.getByTestId("add-books-row").nth(1).click();

    await expect(page).toHaveURL(/\/add-books$/);
    await expect(page.getByTestId("add-books-file-input")).toBeAttached();
    await expect(page.getByTestId("add-books-sheet")).toHaveCount(0);
  });

  test("scrim click dismisses the sheet without navigating", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await page.getByTestId("tabbar-scan").click();
    await expect(page.getByTestId("add-books-sheet")).toBeVisible();

    // Click the scrim outside the sheet body (top-left corner).
    await page.getByTestId("add-books-sheet-scrim").click({
      position: { x: 8, y: 8 },
    });

    await expect(page.getByTestId("add-books-sheet")).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);
  });
});
