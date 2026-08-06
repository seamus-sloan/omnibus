import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The Settings page is a permission-gated sidebar (#1324): each configuration
// group lives behind a `?section=` slug, so tests navigate straight to the
// section whose controls they exercise. The seeded dev user is an admin, so
// every section is present.
const SETTINGS_LIBRARY = "/settings?section=library";
const SETTINGS_METADATA = "/settings?section=metadata";
const SETTINGS_EMAIL = "/settings?section=email";

const ebookInput = (page: Page) => page.getByLabel("Ebook Library Path");
const audiobookInput = (page: Page) =>
  page.getByLabel("Audiobook Library Path");
const scanIntervalInput = (page: Page) =>
  page.getByLabel("Automatic Rescan Interval (hours)");
// Scope the Save button to the library-paths form — the page now also has a
// "Suggestions (Hardcover)" card with its own Save button (F3.3), so an
// unscoped `name: "Save"` would match both and trip Playwright strict mode.
const saveButton = (page: Page) =>
  page.locator("#settings-form").getByRole("button", { name: "Save" });
// The settings page now has two `role=status` regions — the inline form
// status (this locator) and the worker-progress indicator above the Save
// button (`data-testid="worker-status"`, issue #69). Target by testid so
// the assertions stay specific to the form's "Settings saved." line.
const settingsStatus = (page: Page) => page.getByTestId("settings-status");
// F3.3 Hardcover-key card controls — testid-scoped since the card has its
// own "Save"/"Clear" buttons that would otherwise collide with the
// library-paths form's "Save" button (see `saveButton` above).
const hardcoverKeyInput = (page: Page) => page.getByLabel("Hardcover API Key");
const hardcoverSaveButton = (page: Page) => page.getByTestId("hardcover-save");
const hardcoverClearButton = (page: Page) =>
  page.getByTestId("hardcover-clear");
const hardcoverStatus = (page: Page) => page.getByTestId("hardcover-status");
const hardcoverMessage = (page: Page) =>
  page.getByTestId("hardcover-key-status");
// Google Books key card controls — testid-scoped for the same reason as the
// Hardcover card (its own Save/Clear buttons would collide otherwise).
const googleBooksKeyInput = (page: Page) =>
  page.getByLabel("Google Books API Key");
const googleBooksSaveButton = (page: Page) =>
  page.getByTestId("google-books-save");
const googleBooksClearButton = (page: Page) =>
  page.getByTestId("google-books-clear");
const googleBooksStatus = (page: Page) =>
  page.getByTestId("google-books-status");
const googleBooksMessage = (page: Page) =>
  page.getByTestId("google-books-key-status");

test("renders the settings sidebar with every admin section", async ({
  page,
}) => {
  await gotoReady(page, "/settings");

  await expect(page.getByTestId("settings-layout")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "Settings" }),
  ).toBeVisible();
  // The seeded admin sees the per-user items plus every server-config section.
  await expect(page.getByTestId("settings-nav-account")).toBeVisible();
  await expect(page.getByTestId("settings-nav-kindle")).toBeVisible();
  await expect(page.getByTestId("settings-nav-kobo")).toBeVisible();
  await expect(page.getByTestId("settings-nav-library")).toBeVisible();
  await expect(page.getByTestId("settings-nav-metadata")).toBeVisible();
  await expect(page.getByTestId("settings-nav-email")).toBeVisible();
  await expect(page.getByTestId("settings-nav-logs")).toBeVisible();
  await expectNavVisible(page);
});

test("renders the Library Location section controls", async ({ page }) => {
  await gotoReady(page, SETTINGS_LIBRARY);

  await expect(page.getByTestId("settings-nav-library")).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(ebookInput(page)).toBeVisible();
  await expect(audiobookInput(page)).toBeVisible();
  await expect(scanIntervalInput(page)).toBeVisible();
  await expect(saveButton(page)).toBeVisible();
  await expect(page.getByTestId("scan-library")).toBeVisible();
  await expect(settingsStatus(page)).toBeAttached();
  await expect(page.getByTestId("ebook-library-summary")).toBeAttached();
  await expect(page.getByTestId("audiobook-library-summary")).toBeAttached();
});

test("navigates between sections from the sidebar and updates the URL", async ({
  page,
}) => {
  await gotoReady(page, "/settings");

  // Metadata Lookup — the Hardcover + Google Books key cards.
  await page.getByTestId("settings-nav-metadata").click();
  await expect(page).toHaveURL(/\/settings\?section=metadata$/);
  await expect(page.getByTestId("hardcover-key-card")).toBeVisible();
  await expect(page.getByTestId("google-books-key-card")).toBeVisible();

  // Email delivery — the SMTP config card.
  await page.getByTestId("settings-nav-email").click();
  await expect(page).toHaveURL(/\/settings\?section=email$/);
  await expect(page.getByTestId("smtp-card")).toBeVisible();

  // Kindle — the per-user Send-to-Kindle destination address.
  await page.getByTestId("settings-nav-kindle").click();
  await expect(page).toHaveURL(/\/settings\?section=kindle$/);
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();

  // Kobo — the per-user wireless-sync device list.
  await page.getByTestId("settings-nav-kobo").click();
  await expect(page).toHaveURL(/\/settings\?section=kobo$/);
  await expect(page.getByTestId("kobo-devices-card")).toBeVisible();
});

test("saves the SMTP config and shows a configured status", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_EMAIL);

  await page.getByLabel("SMTP Host").fill("smtp.example.com");
  await page.getByLabel("From Email").fill("library@example.com");
  await page.getByLabel("Password").fill("s3cret-pass");

  // Mock the save so the test never depends on a real SMTP relay and never
  // mutates shared server state; the UI renders from the returned masked
  // status. The raw password must never come back — only `password_masked`.
  await page.route("**/api/rpc/smtp", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          configured: true,
          host: "smtp.example.com",
          port: 587,
          username: null,
          from_email: "library@example.com",
          security: "starttls",
          password_masked: "s3…ss",
          source: "settings",
        }),
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/smtp", expectedStatus: 200 },
    async () => page.getByTestId("smtp-save").click(),
  );

  await expect(page.getByTestId("smtp-config-status")).toHaveText(
    "SMTP settings saved.",
  );
  await expect(page.getByTestId("smtp-status")).toContainText("Configured");
});

test("saves library paths and shows a success status", async ({ page }) => {
  await gotoReady(page, SETTINGS_LIBRARY);

  const ebookPath = "/tmp/omnibus-test-ebooks";
  const audiobookPath = "/tmp/omnibus-test-audiobooks";

  await ebookInput(page).fill(ebookPath);
  await audiobookInput(page).fill(audiobookPath);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/settings",
      expectedBody: {
        settings: {
          ebook_library_path: ebookPath,
          audiobook_library_path: audiobookPath,
        },
      },
      expectedStatus: 200,
    },
    async () => saveButton(page).click(),
  );

  await expect(settingsStatus(page)).toHaveText("Settings saved.");
  await expect(settingsStatus(page)).toHaveClass(/success/);
});

test("saves an automatic rescan interval alongside the library paths", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_LIBRARY);

  const ebookPath = "/tmp/omnibus-test-ebooks";
  const audiobookPath = "/tmp/omnibus-test-audiobooks";

  await ebookInput(page).fill(ebookPath);
  await audiobookInput(page).fill(audiobookPath);
  await scanIntervalInput(page).fill("24");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/settings",
      expectedBody: {
        settings: {
          ebook_library_path: ebookPath,
          audiobook_library_path: audiobookPath,
          scan_interval_hours: 24,
        },
      },
      expectedStatus: 200,
    },
    async () => saveButton(page).click(),
  );

  await expect(settingsStatus(page)).toHaveText("Settings saved.");
  await expect(settingsStatus(page)).toHaveClass(/success/);
});

test("shows an error status when the server rejects an invalid rescan interval", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_LIBRARY);

  // Fill every field explicitly (rather than relying on whatever an
  // earlier test left saved) so the expected request body below is
  // deterministic regardless of test order.
  await ebookInput(page).fill("/tmp/whatever");
  await audiobookInput(page).fill("/tmp/whatever-audio");
  await scanIntervalInput(page).fill("1");

  // Force the validation failure server-side (e.g. a stale client vs. a
  // future, stricter floor) rather than relying on the browser's own
  // `min` enforcement, so this exercises the same error-status rendering
  // path as any other 4xx/5xx save failure.
  await page.route("**/api/rpc/settings", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 422,
        contentType: "text/plain",
        body: "scan_interval_hours must be at least 1 (omit to disable)",
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/settings",
      expectedBody: {
        settings: {
          ebook_library_path: "/tmp/whatever",
          audiobook_library_path: "/tmp/whatever-audio",
          scan_interval_hours: 1,
        },
      },
      expectedStatus: 422,
    },
    async () => saveButton(page).click(),
  );

  await expect(settingsStatus(page)).toHaveText("Failed to save settings.");
  await expect(settingsStatus(page)).toHaveClass(/error/);
});

test("shows an error status when saving settings fails", async ({ page }) => {
  await gotoReady(page, SETTINGS_LIBRARY);

  await ebookInput(page).fill("/tmp/whatever");
  await audiobookInput(page).fill("/tmp/whatever-audio");
  // Cleared explicitly (rather than left at whatever an earlier test
  // saved) so the expected request body below omits `scan_interval_hours`
  // deterministically, regardless of test order.
  await scanIntervalInput(page).fill("");

  await page.route("**/api/rpc/settings", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/settings",
      expectedBody: {
        settings: {
          ebook_library_path: "/tmp/whatever",
          audiobook_library_path: "/tmp/whatever-audio",
        },
      },
      expectedStatus: 500,
    },
    async () => saveButton(page).click(),
  );

  await expect(settingsStatus(page)).toHaveText("Failed to save settings.");
  await expect(settingsStatus(page)).toHaveClass(/error/);
});

test("shows an error status when a manual library scan fails", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_LIBRARY);

  await page.route("**/api/rpc/scan-library", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/scan-library", expectedStatus: 500 },
    async () => page.getByTestId("scan-library").click(),
  );

  await expect(settingsStatus(page)).toContainText(
    "Failed to start library scan",
  );
  await expect(settingsStatus(page)).toHaveClass(/error/);
});

// ---------------------------------------------------------------------------
// Action — F3.3 Hardcover key save/clear
// ---------------------------------------------------------------------------

test("saves a Hardcover key, shows a connected status, then clears it", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_METADATA);

  const key = `hc_test_${Date.now()}`;

  await hardcoverKeyInput(page).fill(key);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/hardcover-key",
      expectedBody: { key },
      expectedStatus: 200,
    },
    async () => hardcoverSaveButton(page).click(),
  );

  await expect(hardcoverMessage(page)).toHaveText("Hardcover key saved.");
  await expect(hardcoverMessage(page)).toHaveClass(/success/);
  await expect(hardcoverStatus(page)).toContainText("Connected");
  // The raw key is never echoed back to the client.
  await expect(hardcoverKeyInput(page)).toHaveValue("");
  await expect(hardcoverClearButton(page)).toBeVisible();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/hardcover-key",
      expectedBody: { key: null },
      expectedStatus: 200,
    },
    async () => hardcoverClearButton(page).click(),
  );

  await expect(hardcoverMessage(page)).toHaveText("Hardcover key cleared.");
  await expect(hardcoverMessage(page)).toHaveClass(/success/);
  await expect(hardcoverStatus(page)).toContainText("Not connected");
  await expect(hardcoverClearButton(page)).toHaveCount(0);
});

test("shows an error status when saving the Hardcover key fails", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_METADATA);

  // Only fail the write — the mount-time GET status fetch must still pass
  // through, otherwise the card never renders the fields under test.
  await page.route("**/api/rpc/hardcover-key", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
    }
    return route.continue();
  });

  const key = `hc_test_error_${Date.now()}`;
  await hardcoverKeyInput(page).fill(key);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/hardcover-key",
      expectedBody: { key },
      expectedStatus: 500,
    },
    async () => hardcoverSaveButton(page).click(),
  );

  await expect(hardcoverMessage(page)).toHaveText(
    "Failed to save Hardcover key.",
  );
  await expect(hardcoverMessage(page)).toHaveClass(/error/);
  // The failed save must not clear the admin's typed-in draft.
  await expect(hardcoverKeyInput(page)).toHaveValue(key);
});

test("saves a Google Books key, shows a connected status, then clears it", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_METADATA);

  const key = `AIza_test_${Date.now()}`;

  await googleBooksKeyInput(page).fill(key);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/google-books-key",
      expectedBody: { key },
      expectedStatus: 200,
    },
    async () => googleBooksSaveButton(page).click(),
  );

  await expect(googleBooksMessage(page)).toHaveText("Google Books key saved.");
  await expect(googleBooksMessage(page)).toHaveClass(/success/);
  await expect(googleBooksStatus(page)).toContainText("Connected");
  // The raw key is never echoed back to the client.
  await expect(googleBooksKeyInput(page)).toHaveValue("");
  await expect(googleBooksClearButton(page)).toBeVisible();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/google-books-key",
      expectedBody: { key: null },
      expectedStatus: 200,
    },
    async () => googleBooksClearButton(page).click(),
  );

  await expect(googleBooksMessage(page)).toHaveText(
    "Google Books key cleared.",
  );
  await expect(googleBooksMessage(page)).toHaveClass(/success/);
  await expect(googleBooksStatus(page)).toContainText("Not connected");
  await expect(googleBooksClearButton(page)).toHaveCount(0);
});

test("shows an error status when saving the Google Books key fails", async ({
  page,
}) => {
  await gotoReady(page, SETTINGS_METADATA);

  // Only fail the write — the mount-time GET status fetch must still pass
  // through, otherwise the card never renders the fields under test.
  await page.route("**/api/rpc/google-books-key", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
    }
    return route.continue();
  });

  const key = `AIza_test_error_${Date.now()}`;
  await googleBooksKeyInput(page).fill(key);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/google-books-key",
      expectedBody: { key },
      expectedStatus: 500,
    },
    async () => googleBooksSaveButton(page).click(),
  );

  await expect(googleBooksMessage(page)).toHaveText(
    "Failed to save Google Books key.",
  );
  await expect(googleBooksMessage(page)).toHaveClass(/error/);
  // The failed save must not clear the admin's typed-in draft.
  await expect(googleBooksKeyInput(page)).toHaveValue(key);
});
