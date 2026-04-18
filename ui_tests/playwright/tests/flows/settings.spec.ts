import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible } from "../utils/nav";

test("renders the settings page layout", async ({ page }) => {
  await page.goto("/settings");

  await expect(page.locator("h1")).toHaveText("Settings");
  await expect(page.locator("#settings-form")).toBeVisible();
  await expect(page.locator("#ebook-library-path")).toBeVisible();
  await expect(page.locator("#audiobook-library-path")).toBeVisible();
  await expect(page.locator("#settings-status")).toBeAttached();
  await expect(page.locator("#ebook-library-contents")).toBeVisible();
  await expect(page.locator("#audiobook-library-contents")).toBeVisible();
  await expectNavVisible(page);
});

test("saves library paths and shows a success status", async ({ page }) => {
  await page.goto("/settings");

  const ebookPath = "/tmp/omnibus-test-ebooks";
  const audiobookPath = "/tmp/omnibus-test-audiobooks";

  await page.locator("#ebook-library-path").fill(ebookPath);
  await page.locator("#audiobook-library-path").fill(audiobookPath);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/settings",
      expectedBody: {
        ebook_library_path: ebookPath,
        audiobook_library_path: audiobookPath,
      },
      expectedStatus: 200,
    },
    async () => page.locator("#settings-form button[type=submit]").click(),
  );

  await expect(page.locator("#settings-status")).toHaveText("Settings saved.");
  await expect(page.locator("#settings-status")).toHaveClass(/success/);
});

test("shows an error status when saving settings fails", async ({ page }) => {
  await page.goto("/settings");

  await page.locator("#ebook-library-path").fill("/tmp/whatever");
  await page.locator("#audiobook-library-path").fill("/tmp/whatever-audio");

  await page.route("**/api/settings", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/settings",
      expectedBody: {
        ebook_library_path: "/tmp/whatever",
        audiobook_library_path: "/tmp/whatever-audio",
      },
      expectedStatus: 500,
    },
    async () => page.locator("#settings-form button[type=submit]").click(),
  );

  await expect(page.locator("#settings-status")).toHaveText("Failed to save settings.");
  await expect(page.locator("#settings-status")).toHaveClass(/error/);
});
