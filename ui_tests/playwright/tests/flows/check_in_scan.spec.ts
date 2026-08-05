import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";

// Headless Chromium refuses `getUserMedia` without a fake device, so every run
// here lands on the permission-denied branch — which is exactly the fallback
// contract worth pinning (a scanner nobody can grant the camera to must still
// reach manual entry). The happy-path decode needs a real camera and is
// covered by the decoder's own unit tests plus device QA.

// The camera is never the web flow's front door — it sits one explicit button
// away from the lookup screen, which `check_in_lookup.spec.ts` covers.
async function reachScanner(page: Page): Promise<void> {
  await gotoReady(page, "/check-in");
  await page.getByTestId("check-in-scan-instead").click();
  await expect(page.getByTestId("check-in-scan")).toBeVisible();
}

test.describe("check-in scanner", () => {
  test("renders the scan layout", async ({ page }) => {
    await reachScanner(page);
    await expectNavVisible(page);

    await expect(
      page.getByRole("heading", { name: "Scan a barcode" }),
    ).toBeVisible();
    // The viewfinder and its escape hatch mount together, so a reader who
    // opened the camera always has the keypad within reach.
    await expect(page.getByTestId("barcode-scanner")).toBeVisible();
    await expect(page.getByTestId("barcode-scanner-manual")).toBeVisible();
  });

  test("a camera the browser won't grant lands on manual entry", async ({
    page,
  }) => {
    await reachScanner(page);

    // The glue reports the refusal, and the status line names the fallback.
    await expect(page.getByTestId("barcode-scanner-status")).toContainText(
      "ISBN",
    );
    // With no camera coming, the dead viewfinder is dropped and the keypad
    // link is promoted to the primary action.
    await expect(page.getByTestId("barcode-scanner-video")).toBeHidden();
    await expect(page.getByTestId("barcode-scanner-manual")).toHaveClass(
      /primary/,
    );

    await page.getByTestId("barcode-scanner-manual").click();
    await expect(page.getByTestId("check-in-entry")).toBeVisible();
  });

  test("the keypad gates Find book on a valid check digit", async ({
    page,
  }) => {
    await reachScanner(page);
    await page.getByTestId("barcode-scanner-manual").click();

    const submit = page.getByTestId("check-in-submit");
    await expect(submit).toBeDisabled();

    // Twelve of thirteen digits: complete-looking, but not yet an ISBN.
    for (const key of "978044101359") {
      await page.getByTestId(`check-in-key-${key}`).click();
    }
    await expect(page.getByTestId("check-in-isbn-progress")).toHaveAttribute(
      "data-filled",
      "12",
    );
    await expect(submit).toBeDisabled();

    // Thirteenth digit, wrong: right length, so only the check digit can
    // catch it.
    await page.getByTestId("check-in-key-7").click();
    await expect(submit).toBeDisabled();

    // Backspace and enter the real check digit.
    await page.getByTestId("check-in-key-⌫").click();
    await page.getByTestId("check-in-key-3").click();
    await expect(page.getByTestId("check-in-isbn")).toHaveValue(
      "9780441013593",
    );
    await expect(submit).toBeEnabled();
  });

  test("returns to the scanner from manual entry", async ({ page }) => {
    await reachScanner(page);
    await page.getByTestId("barcode-scanner-manual").click();
    await expect(page.getByTestId("check-in-entry")).toBeVisible();

    await page.getByTestId("check-in-scan-instead").click();
    await expect(page.getByTestId("check-in-scan")).toBeVisible();
  });
});
