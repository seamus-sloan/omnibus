import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The flow's front door: ISBN entry and title search on one screen, with the
// camera behind an explicit button. The provider RPCs are mocked so these
// assertions don't ride on ambient server config — real provider behaviour is
// covered by the `omnibus_db` wiremock suites.

const ISBN = "9780441013593";
const KEY_STATUS_URL = "/api/rpc/scan/google-books-configured";

async function mockJsonPost(
  page: Page,
  url: string | RegExp,
  body: unknown,
): Promise<void> {
  await page.route(url, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(body),
        })
      : route.continue(),
  );
}

// The provider disclaimer keys off the non-admin-gated configured flag
// (`POST /api/rpc/scan/google-books-configured`). Mock it so the note's two
// states don't ride on ambient server config (another worker's settings save
// could otherwise flip it mid-run). It's a read that POSTs because it's a
// Dioxus server function, so its `expectMutation` wrappers assert status only
// and raise the timeout — the call is triggered by a page load, not a click.
async function mockGoogleBooksKey(
  page: Page,
  configured: boolean,
): Promise<void> {
  await page.route("**/api/rpc/scan/google-books-configured", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(configured),
        })
      : route.continue(),
  );
}

test.describe("check-in lookup", () => {
  test("renders the lookup layout", async ({ page }) => {
    await gotoReady(page, "/check-in");
    await expectNavVisible(page);

    await expect(page.getByTestId("check-in-lookup")).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Check in a book" }),
    ).toBeVisible();
    // Both ways in are on screen at once — no mode switch to find either.
    await expect(page.getByLabel("ISBN")).toBeVisible();
    await expect(page.getByLabel("Title")).toBeVisible();
    await expect(page.getByTestId("check-in-scan-instead")).toBeVisible();
    // Nothing typed yet: neither submit can fire, and no stale empty state.
    await expect(page.getByTestId("check-in-submit")).toBeDisabled();
    await expect(page.getByTestId("check-in-search-submit")).toBeDisabled();
    await expect(page.getByTestId("check-in-search-empty")).toHaveCount(0);
  });

  test("opening the flow mounts no scanner, so no camera starts", async ({
    page,
  }) => {
    await gotoReady(page, "/check-in");

    // The camera-opening effect runs on the scanner component's mount, so an
    // unmounted scanner is what keeps a webcam dark until someone asks for it.
    await expect(page.getByTestId("check-in-lookup")).toBeVisible();
    await expect(page.getByTestId("barcode-scanner")).toHaveCount(0);
    await expect(page.getByTestId("barcode-scanner-video")).toHaveCount(0);
  });

  test("the scan button is what reveals the camera", async ({ page }) => {
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-scan-instead").click();

    await expect(page.getByTestId("check-in-scan")).toBeVisible();
    await expect(page.getByTestId("barcode-scanner")).toBeVisible();
    await expect(page.getByTestId("check-in-lookup")).toHaveCount(0);
  });

  test("Find book stays disabled until the check digit validates", async ({
    page,
  }) => {
    await gotoReady(page, "/check-in");
    const submit = page.getByTestId("check-in-submit");
    await expect(submit).toBeDisabled();

    // Right length, wrong check digit — the case a shape-only gate misses.
    await page.getByTestId("check-in-isbn").fill("9780441013539");
    await expect(submit).toBeDisabled();

    await page.getByTestId("check-in-isbn").fill(ISBN);
    await expect(submit).toBeEnabled();
  });

  test("an unresolved ISBN routes back to the lookup screen's title search", async ({
    page,
  }) => {
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "unresolved",
    });
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);

    // Dioxus keys a server-fn body by parameter name, so `req` wraps the struct.
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedBody: { req: { isbn: ISBN } },
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );

    await expect(page.getByTestId("check-in-unresolved")).toBeVisible();
    await page.getByTestId("check-in-search-instead").click();

    await expect(page.getByTestId("check-in-lookup")).toBeVisible();
    await expect(page.getByLabel("Title")).toBeVisible();
  });

  test("a failed lookup surfaces the error without leaving the fields", async ({
    page,
  }) => {
    await page.route(/\/api\/rpc\/scan\/resolve$/, (route) =>
      route.request().method() === "POST"
        ? route.fulfill({
            status: 500,
            contentType: "text/plain",
            body: "forced failure",
          })
        : route.continue(),
    );
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);

    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedBody: { req: { isbn: ISBN } },
        expectedStatus: 500,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );

    // The reader is dropped back where they typed, digits intact to correct —
    // never onto the keypad screen they never asked for.
    await expect(page.getByTestId("check-in-error")).toBeVisible();
    await expect(page.getByTestId("check-in-lookup")).toBeVisible();
    await expect(page.getByTestId("check-in-isbn")).toHaveValue(ISBN);
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
    await mockGoogleBooksKey(page, false);
    await expectMutation(
      page,
      {
        method: "POST",
        url: KEY_STATUS_URL,
        expectedStatus: 200,
        timeout: 15_000,
      },
      async () => gotoReady(page, "/check-in"),
    );

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
    await mockGoogleBooksKey(page, true);
    await expectMutation(
      page,
      {
        method: "POST",
        url: KEY_STATUS_URL,
        expectedStatus: 200,
        timeout: 15_000,
      },
      async () => gotoReady(page, "/check-in"),
    );

    await expect(page.getByTestId("check-in-lookup")).toBeVisible();
    await expect(page.getByTestId("check-in-google-books-note")).toHaveCount(0);
  });
});
