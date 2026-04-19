import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

const ebookInput = (page: Page) => page.getByLabel("Ebook Library Path");
const audiobookInput = (page: Page) => page.getByLabel("Audiobook Library Path");
const saveButton = (page: Page) => page.getByRole("button", { name: "Save" });
const settingsStatus = (page: Page) => page.getByRole("status");

test("renders the settings page layout", async ({ page }) => {
  await page.goto("/settings");

  await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
  await expect(ebookInput(page)).toBeVisible();
  await expect(audiobookInput(page)).toBeVisible();
  await expect(saveButton(page)).toBeVisible();
  await expect(settingsStatus(page)).toBeAttached();
  await expect(page.getByTestId("ebook-library-summary")).toBeAttached();
  await expect(page.getByTestId("audiobook-library-summary")).toBeAttached();
  await expectNavVisible(page);
});

test("saves library paths and shows a success status", async ({ page }) => {
  await gotoReady(page, "/settings");

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

test("shows an error status when saving settings fails", async ({ page }) => {
  await gotoReady(page, "/settings");

  await ebookInput(page).fill("/tmp/whatever");
  await audiobookInput(page).fill("/tmp/whatever-audio");

  await page.route("**/api/rpc/settings", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
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
