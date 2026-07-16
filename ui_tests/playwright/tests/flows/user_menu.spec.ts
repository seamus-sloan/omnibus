import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The user menu replaces the old top-bar cluster (ThemeToggle, Settings
// link, Log out button) with a single avatar trigger that opens a dropdown.
// Most rows in the dropdown are forward-looking stubs (disabled <a>); real
// wiring covers recent progress, Settings, Sign out, and theme selection.

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

async function recentPoint(
  request: import("@playwright/test").APIRequestContext,
  format: "epub" | "audio",
) {
  const target = FIXTURE_BOOKS[1];
  const uuid = await fetchBookUuidByTitle(request, target.title);
  const response = await request.get("/api/rpc/ebooks");
  expect(response.status()).toBe(200);
  const books = ((await response.json()) as {
    books: Record<string, unknown>[];
  }).books;
  const book = books.find((candidate) => candidate.unique_identifier === uuid);
  if (!book) {
    throw new Error(`book ${JSON.stringify(target.title)} disappeared after lookup`);
  }

  return {
    uuid,
    title: target.title,
    point: {
      record: {
        book_uuid: uuid,
        format,
        epub_cfi: format === "epub" ? "epubcfi(/6/2)" : null,
        audio_position_seconds: format === "audio" ? 90 : null,
        updated_at: 123,
      },
      book,
      total_duration_seconds: null,
      chapter_number: null,
      chapter_count: null,
    },
  };
}

test("renders the user menu trigger after login", async ({ page }) => {
  await gotoReady(page, "/");

  const trigger = page.getByTestId("user-menu-trigger");
  await expect(trigger).toBeVisible();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");

  // The old top-bar Log out button is gone — it now lives inside the menu
  // and should not be reachable while the menu is closed.
  await expect(page.getByTestId("logout-button")).toHaveCount(0);
});

test("opens and closes the user menu", async ({ page }) => {
  await gotoReady(page, "/");

  const trigger = page.getByTestId("user-menu-trigger");
  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");

  // Real actions are present.
  await expect(page.getByRole("link", { name: "Settings" })).toBeVisible();
  await expect(page.getByTestId("logout-button")).toBeVisible();
  await expect(page.getByTestId("theme-dark")).toBeVisible();
  await expect(page.getByTestId("theme-light")).toBeVisible();

  // Version line at the bottom of the panel (#1055) — a compile-time
  // constant, so it's always present regardless of OMNIBUS_VERSION.
  await expect(page.getByTestId("user-menu-version")).toBeVisible();
  await expect(page.getByTestId("user-menu-version")).toHaveText(/^v\d+\.\d+\.\d+$/);

  // Close via the transparent scrim (click outside the panel).
  await page.getByTestId("user-menu-scrim").click();
  await expect(page.getByTestId("logout-button")).toHaveCount(0);

  // Re-open and close via ESC. Dispatch the keypress directly to the
  // panel locator so the test doesn't race the onmounted focus call.
  await trigger.click();
  const panel = page.getByTestId("user-menu-panel");
  await expect(panel).toBeVisible();
  await panel.press("Escape");
  await expect(page.getByTestId("logout-button")).toHaveCount(0);
});

test("Settings link routes to the settings page", async ({ page }) => {
  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();
  await page.getByRole("link", { name: "Settings" }).click();
  await expect(page).toHaveURL(/\/settings$/);
});

for (const sample of [
  { format: "epub" as const, action: "Continue reading", path: "read" },
  { format: "audio" as const, action: "Continue listening", path: "listen" },
]) {
  test(`shows the latest ${sample.format} and its resume destination`, async ({
    page,
    request,
  }) => {
    const latest = await recentPoint(request, sample.format);
    await page.route("**/api/rpc/progress/recent", async (route) => {
      await route.fulfill({ status: 200, json: [latest.point] });
    });

    await gotoReady(page, "/");
    await page.getByTestId("user-menu-trigger").click();

    const card = page.getByRole("link", {
      name: `${sample.action} ${latest.title}`,
    });
    await expect(card).toBeVisible();
    await expect(card).toHaveAttribute("href", `/${sample.path}/${latest.uuid}`);
  });
}

test("shows an empty state when no book has progress", async ({ page }) => {
  await page.route("**/api/rpc/progress/recent", async (route) => {
    await route.fulfill({ status: 200, json: [] });
  });

  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();

  await expect(page.getByText("Nothing in progress")).toBeVisible();
  await expect(page.getByText("Piranesi")).toHaveCount(0);
});

test("surfaces a recent-progress fetch failure", async ({ page }) => {
  await page.route("**/api/rpc/progress/recent", async (route) => {
    await route.fulfill({ status: 500, body: "forced failure" });
  });

  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();

  await expect(page.getByRole("alert")).toHaveText("Unable to load reading progress.");
});

test("Sign out clears the session and routes to /login", async ({ page }) => {
  // Mock /api/auth/logout instead of letting it hit the real endpoint:
  //   1. A real logout would invalidate the globalSetup-minted session
  //      that every parallel test relies on — they'd all redirect to
  //      /login mid-run with a clobbered cookie.
  //   2. The realistic alternative (fresh login per test) burns the
  //      /api/auth/login per-IP rate-limit budget that auth.spec.ts
  //      already saturates in CI.
  // The backend logout path is covered by `server::auth` integration
  // tests; this spec is the UI contract — request fires, UI navigates.
  await page.route("**/api/auth/logout", async (route) => {
    await route.fulfill({ status: 200, contentType: "text/plain", body: "" });
  });

  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();

  await expectMutation(
    page,
    { method: "POST", url: "/api/auth/logout", expectedStatus: 200 },
    async () => page.getByTestId("logout-button").click(),
  );

  await expect(page).toHaveURL(/\/login$/);
});
