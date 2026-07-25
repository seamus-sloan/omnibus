import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The scanner's Google Books disclaimer keys off the admin-only masked key
// status (`GET /api/rpc/google-books-key`). Mock it so the note's two states
// don't ride on ambient server config (another worker's settings save could
// otherwise flip it mid-run).
async function mockGoogleBooksKey(
  page: Page,
  configured: boolean,
): Promise<void> {
  await page.route("**/api/rpc/google-books-key", (route) => {
    if (route.request().method() !== "GET") return route.continue();
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(
        configured
          ? { configured: true, masked: "AIza…f3a9", source: "settings" }
          : { configured: false, masked: null, source: "none" },
      ),
    });
  });
}

// Headless Chromium refuses `getUserMedia` without a fake device, so every run
// here lands on the permission-denied branch — which is exactly the fallback
// contract worth pinning (a scanner nobody can grant the camera to must still
// reach manual entry). The happy-path decode needs a real camera and is
// covered by the decoder's own unit tests plus device QA.
test.describe("check-in scanner", () => {
  test("renders the scan layout", async ({ page }) => {
    await gotoReady(page, "/check-in");
    await expectNavVisible(page);

    await expect(page.getByTestId("check-in-scan")).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Scan a barcode" }),
    ).toBeVisible();
    // The viewfinder and its escape hatch are both part of the first paint —
    // SSR renders the same tree the client hydrates.
    await expect(page.getByTestId("barcode-scanner")).toBeVisible();
    await expect(page.getByTestId("barcode-scanner-manual")).toBeVisible();
  });

  test("a camera the browser won't grant lands on manual entry", async ({
    page,
  }) => {
    await gotoReady(page, "/check-in");

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
    await gotoReady(page, "/check-in");
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
    await gotoReady(page, "/check-in");
    await page.getByTestId("barcode-scanner-manual").click();
    await expect(page.getByTestId("check-in-entry")).toBeVisible();

    await page.getByTestId("check-in-scan-instead").click();
    await expect(page.getByTestId("check-in-scan")).toBeVisible();
  });

  test("the cancel button exits the flow back to the library", async ({
    page,
  }) => {
    await gotoReady(page, "/check-in");
    await expect(page.getByTestId("check-in")).toBeVisible();

    await page.getByTestId("check-in-close").click();

    await expect(page).toHaveURL(/\/$/);
    await expect(
      page.getByRole("heading", { level: 1, name: "Your Library" }),
    ).toBeVisible();
    await expect(page.getByTestId("check-in")).toHaveCount(0);
  });

  test("shows the Open Library fallback note when no Google Books key is set", async ({
    page,
  }) => {
    const responded = page.waitForResponse((r) =>
      r.url().includes("/api/rpc/google-books-key"),
    );
    await mockGoogleBooksKey(page, false);
    await gotoReady(page, "/check-in");
    await responded;

    await expect(page.getByTestId("check-in-google-books-note")).toContainText(
      "Without a Google Books API key",
    );
    await expect(page.getByTestId("check-in-google-books-note")).toContainText(
      "Set up your Google Books API key in Settings",
    );
  });

  test("hides the fallback note when a Google Books key is configured", async ({
    page,
  }) => {
    const responded = page.waitForResponse((r) =>
      r.url().includes("/api/rpc/google-books-key"),
    );
    await mockGoogleBooksKey(page, true);
    await gotoReady(page, "/check-in");
    await responded;

    await expect(page.getByTestId("check-in-scan")).toBeVisible();
    await expect(page.getByTestId("check-in-google-books-note")).toHaveCount(0);
  });
});
