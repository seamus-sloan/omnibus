import { resolve } from "node:path";

import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { gotoReady, expectNavVisible } from "../utils/nav";
import { fixturesDir } from "../utils/seed";

// A committed EPUB fixture to feed the file input. Any valid EPUB works — the
// inspect endpoint parses its embedded metadata.
const SAMPLE_EPUB = resolve(fixturesDir(), "generated", "beta.epub");

const fileInput = (page: import("@playwright/test").Page) =>
  page.getByTestId("add-books-file-input");
const status = (page: import("@playwright/test").Page) =>
  page.getByTestId("add-books-status");

test("renders the add-books layout", async ({ page }) => {
  await gotoReady(page, "/add-books");

  await expect(page.getByRole("heading", { name: "Add books" })).toBeVisible();
  await expect(fileInput(page)).toBeVisible();
  // The audiobook type is present but stubbed out.
  await expect(page.getByTestId("add-books-type-audiobook")).toBeDisabled();
  await expectNavVisible(page);
});

test("auto-fills the editable form from an uploaded EPUB", async ({ page }) => {
  await gotoReady(page, "/add-books");

  // Selecting a file kicks off the inspect round-trip; the form appears
  // pre-filled with the embedded metadata.
  await expectMutation(
    page,
    { method: "POST", url: "/api/uploads/ebooks/inspect", expectedStatus: 200 },
    async () => fileInput(page).setInputFiles(SAMPLE_EPUB),
  );

  await expect(page.getByTestId("add-books-submit")).toBeVisible();
  // The title field is populated from the embedded metadata (non-empty).
  await expect(page.getByLabel("Title")).not.toHaveValue("");
});

test("surfaces an error when inspect fails", async ({ page }) => {
  await gotoReady(page, "/add-books");

  // Force the inspect call to fail server-side.
  await page.route("**/api/uploads/ebooks/inspect", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await expectMutation(
    page,
    { method: "POST", url: "/api/uploads/ebooks/inspect", expectedStatus: 500 },
    async () => fileInput(page).setInputFiles(SAMPLE_EPUB),
  );

  // The status region surfaces the failure, and the confirm form never appears.
  await expect(status(page)).toBeVisible();
  await expect(status(page)).toHaveClass(/error/);
  await expect(page.getByTestId("add-books-submit")).toHaveCount(0);
});
