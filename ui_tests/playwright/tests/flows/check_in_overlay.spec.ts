import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The check-in flow is presented as a centered overlay over a blurred page.
// The top-nav trigger renders at desktop width, so this runs at the default
// (desktop) viewport. The full-page `/check-in` deep-link fallback is covered
// by `check_in_lookup.spec.ts`, the camera by `check_in_scan.spec.ts`.
test.describe("check-in overlay", () => {
  test("the top-nav Check in button opens the overlay without navigating", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await expectNavVisible(page);
    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);

    await page.getByTestId("check-in-button").click();

    // Floats in place over the current page — no route change.
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();
    await expect(page.getByTestId("check-in")).toBeVisible();
    // Opens on the fields, not the camera — raising the overlay from the top
    // nav must never start a webcam.
    await expect(
      page.getByRole("heading", { name: "Check in a book" }),
    ).toBeVisible();
    await expect(page.getByTestId("barcode-scanner")).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);

    // The overlay shows exactly one close affordance: its own dismiss button.
    // The flow's own cancel × (a Link to /) is hidden inside the overlay so it
    // doesn't double up or navigate the user away on close.
    await expect(page.getByTestId("check-in-overlay-close")).toBeVisible();
    await expect(page.getByTestId("check-in-close")).toBeHidden();
  });

  test("the close button dismisses the overlay", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("check-in-button").click();
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();

    await page.getByTestId("check-in-overlay-close").click();

    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);
  });

  test("clicking the scrim outside the card dismisses the overlay", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await page.getByTestId("check-in-button").click();
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();

    // Top-left corner is scrim, well outside the centered card.
    await page.getByTestId("check-in-overlay-scrim").click({
      position: { x: 8, y: 8 },
    });

    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
  });

  test("Escape dismisses the overlay", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("check-in-button").click();
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();

    // Opening the overlay moves focus into the dialog panel, so a global
    // Escape (no prior click) dismisses it — the real keyboard-user contract.
    await expect(page.getByTestId("check-in-overlay-panel")).toBeFocused();
    await page.keyboard.press("Escape");

    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
  });
});
