import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The user menu replaces the old top-bar cluster (ThemeToggle, Settings
// link, Log out button) with a single avatar trigger that opens a dropdown.
// Some rows are forward-looking stubs (disabled <a>); real wiring covers
// recent progress, Settings, Switch user, Sign out, and theme selection.
// Account, "Admin · server health", and Notifications were folded into the
// unified Settings sidebar (#1324), so they no longer appear here.

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

async function recentPoint(
  request: import("@playwright/test").APIRequestContext,
  format: "epub" | "audio",
  bookFileId: number | null = null,
) {
  const target = FIXTURE_BOOKS[1]!;
  const uuid = await fetchBookUuidByTitle(request, target.title);
  const response = await request.get("/api/rpc/ebooks");
  expect(response.status()).toBe(200);
  const books = (
    (await response.json()) as {
      books: Record<string, unknown>[];
    }
  ).books;
  const book = books.find((candidate) => candidate.unique_identifier === uuid);
  if (!book) {
    throw new Error(
      `book ${JSON.stringify(target.title)} disappeared after lookup`,
    );
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
        // Which of the book's files the position was taken in — the server
        // re-resolves this on every resume point, so it is always a live id.
        book_file_id: bookFileId,
        updated_at: 123,
        // Server responses always populate this (issue #1362); the mock
        // must match the real wire shape or the client's deserialization
        // of the recent-progress response fails and the resume card never
        // renders.
        client_updated_at: 123,
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
  await expect(page.getByTestId("user-menu-version")).toHaveText(
    /^v\d+\.\d+\.\d+$/,
  );

  // Close by clicking a real viewport coordinate well below the header,
  // not the scrim locator's centre. `.atrium-topbar` sets
  // `backdrop-filter`, which makes it the containing block for the
  // `position: fixed` scrim nested inside it — a scrim that collapsed to
  // the header strip still satisfies a centre-click while leaving the rest
  // of the page uncovered, so only mouse coordinates catch the regression.
  await page.mouse.click(40, 500);
  await expect(page.getByTestId("logout-button")).toHaveCount(0);
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  // The scrim swallows the click — it must not fall through to the page.
  await expect.poll(async () => new URL(page.url()).pathname).toBe("/");

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
  // `Route::Settings { section: None }` serializes with a trailing `?` (dioxus
  // renders the absent optional query param), so allow it — the section-less
  // URL lands on the default Account section either way.
  await expect(page).toHaveURL(/\/settings\??$/);
});

test("the dropdown no longer surfaces the folded-in rows (#1324)", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();
  const panel = page.getByTestId("user-menu-panel");
  await expect(panel).toBeVisible();
  // Account / Admin · server health / Notifications now live under Settings.
  // Scope to the open panel so unrelated page text can't flake the assertion.
  await expect(panel.getByRole("link", { name: "Account" })).toHaveCount(0);
  await expect(
    panel.getByRole("link", { name: "Admin · server health" }),
  ).toHaveCount(0);
  await expect(panel.getByText("Notifications")).toHaveCount(0);
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
    const panel = page.getByTestId("user-menu-panel");

    // Only the action button resumes reading/listening.
    const action = panel.getByRole("link", {
      name: `${sample.action} ${latest.title}`,
    });
    await expect(action).toBeVisible();
    // The `/listen/:uuid?:file_id` route serializes an unset file_id as a bare
    // trailing `?`, so tolerate it (the `/read` destination stays clean).
    await expect(action).toHaveAttribute(
      "href",
      new RegExp(`^/${sample.path}/${latest.uuid}\\??$`),
    );

    // The cover instead routes to the book detail page.
    const cover = panel.getByRole("link", {
      name: `View details for ${latest.title}`,
    });
    await expect(cover).toHaveAttribute("href", `/books/${latest.uuid}`);
  });
}

test("Continue listening resumes the audiobook file the position was taken in", async ({
  page,
  request,
}) => {
  // A book can carry more than one audiobook (two narrations, an abridged
  // edition). A bare `/listen/:uuid` resolves to the first audio file by
  // ordinal, so the CTA used to reopen the wrong narration seeked to the
  // right narration's timestamp.
  const latest = await recentPoint(request, "audio", 917);
  await page.route("**/api/rpc/progress/recent", async (route) => {
    await route.fulfill({ status: 200, json: [latest.point] });
  });

  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();

  await expect(
    page.getByTestId("user-menu-now-reading-action"),
  ).toHaveAttribute("href", `/listen/${latest.uuid}?file_id=917`);
});

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

  await expect(page.getByRole("alert")).toHaveText(
    "Unable to load reading progress.",
  );
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

test("the Edit link opens the Account settings section", async ({ page }) => {
  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();

  await page.getByRole("link", { name: "Edit" }).click();

  await expect(page).toHaveURL(/\/settings\?section=account$/);
  await expect(page.getByTestId("account-profile-card")).toBeVisible();
});
