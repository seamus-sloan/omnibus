import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// "Fetch from Hardcover" panel on the metadata-edit page (#984): an
// on-demand lookup that previews Hardcover's canonical title/author/
// description/series/ISBN-13 next to the current form values. Applying only
// stages the chosen fields into the form's signals — the existing Save
// button (already covered by `metadata_edit.spec.ts`) is what writes them
// through as metadata overrides.
//
// The panel is hidden entirely when no Hardcover key is configured, and the
// fixtures/CI environment has none, so every mocked test here intercepts
// both the availability check and the fetch itself via `page.route` — the
// same approach `fetch_summary.spec.ts` and `suggestions.spec.ts` use for the
// same reason (no live external API in CI, and the key is shared global
// state other specs also mutate).
//
// `standalone-forest` (Henri Poincare, "La Foret des Algorithmes") is used
// because it's read by no other spec's override-mutating tests — see the
// fixture-isolation note in `.claude/rules/04-playwright.md`.

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "standalone-forest")!;

const AVAILABLE_URL = "**/api/rpc/ebook/hardcover-fetch/available";
const FETCH_URL = "**/api/rpc/ebook/hardcover-fetch";

async function mockAvailable(page: Page, available: boolean): Promise<void> {
  await page.route(AVAILABLE_URL, (route) =>
    route.request().method() === "GET"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(available),
        })
      : route.continue(),
  );
}

type MockFetchResult =
  | { status: "not_configured" }
  | { status: "not_found" }
  | {
      status: "found";
      title?: string | null;
      authors?: string[];
      description?: string | null;
      series?: string | null;
      series_index?: string | null;
      isbn13?: string | null;
    };

async function mockFetchResult(
  page: Page,
  result: MockFetchResult,
): Promise<void> {
  await page.route(FETCH_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(result),
        })
      : route.continue(),
  );
}

/** Click the fetch button and await the POST it fires, so DOM assertions
 * don't race the network (per `.claude/rules/04-playwright.md`). */
async function clickFetch(page: Page): Promise<void> {
  const fetched = page.waitForResponse(
    (r) =>
      r.url().includes("/api/rpc/ebook/hardcover-fetch") &&
      r.request().method() === "POST",
  );
  await page.getByTestId("hardcover-fetch-btn").click();
  await fetched;
}

const FOUND_RESULT: MockFetchResult = {
  status: "found",
  title: "La Foret des Algorithmes: Deuxieme Edition",
  authors: ["Henri Poincare", "Emmy Noether"],
  description: "A revised survey of recursive structures in the deep woods.",
  series: "Mathematique Moderne",
  series_index: "2",
  isbn13: "9781234567897",
};

// The "select all -> save -> revert" test below temporarily renames the
// fixture book server-side; every other test in this file looks the book up
// by its original title. `describe.serial` pins them to one worker (same
// reasoning as `metadata_edit.spec.ts`'s serial block) so a save from one
// test can never be mid-flight while another looks the book up.
test.describe
  .serial("metadata edit hardcover fetch flow", () => {
    // ---------------------------------------------------------------------------
    // Hidden when not configured (AC3)
    // ---------------------------------------------------------------------------

    test("hides the Fetch from Hardcover action when no key is configured", async ({
      page,
      request,
    }) => {
      // Mocked rather than relying on the fixtures/CI server's ambient
      // Hardcover-key state (per `.claude/rules/03-unit-testing.md`'s "no
      // ambient environment" principle) — a locally or future-CI configured
      // `HARDCOVER_API_KEY` would otherwise legitimately show the button and
      // fail this test.
      const id = await fetchBookIdByTitle(request, TARGET.title);
      await mockAvailable(page, false);
      await gotoReady(page, `/books/${id}/edit`);

      await expect(page.getByTestId("hardcover-fetch-btn")).toHaveCount(0);
    });

    // ---------------------------------------------------------------------------
    // No match on Hardcover (AC3)
    // ---------------------------------------------------------------------------

    test("shows an explicit not-found state when Hardcover has no match", async ({
      page,
      request,
    }) => {
      const id = await fetchBookIdByTitle(request, TARGET.title);
      await mockAvailable(page, true);
      await gotoReady(page, `/books/${id}/edit`);
      await expect(page.getByTestId("hardcover-fetch-btn")).toBeVisible();

      await mockFetchResult(page, { status: "not_found" });
      await clickFetch(page);

      await expect(page.getByTestId("hardcover-fetch-not-found")).toHaveText(
        "No match found on Hardcover for this book.",
      );
    });

    // ---------------------------------------------------------------------------
    // Fetch -> preview -> per-field apply (only Title checked)
    // ---------------------------------------------------------------------------

    test("applies only the checked field from the preview", async ({
      page,
      request,
    }) => {
      const id = await fetchBookIdByTitle(request, TARGET.title);
      await mockAvailable(page, true);
      await gotoReady(page, `/books/${id}/edit`);

      await mockFetchResult(page, FOUND_RESULT);
      await clickFetch(page);

      // Preview shows current-vs-fetched for every returned field.
      const preview = page.getByTestId("hardcover-fetch-preview");
      await expect(preview).toBeVisible();
      await expect(
        page.getByTestId("hardcover-fetch-title-current"),
      ).toHaveText(TARGET.title);
      await expect(
        page.getByTestId("hardcover-fetch-title-fetched"),
      ).toHaveText("La Foret des Algorithmes: Deuxieme Edition");

      // Check only the Title row, then apply.
      await page.getByTestId("hardcover-fetch-title-checkbox").check();
      await page.getByTestId("hardcover-fetch-apply").click();

      // Title updated; Author(s) and Description (unchecked) stayed as-is.
      await expect(page.getByLabel("Title")).toHaveValue(
        "La Foret des Algorithmes: Deuxieme Edition",
      );
      await expect(
        page.locator(".me-chip-item").getByText(TARGET.authors[0]!),
      ).toBeVisible();
      await expect(page.locator("#me-description")).toHaveValue("");

      // Staged only — no save fired, still on the edit form.
      await expect(page.getByTestId("me-save")).toBeEnabled();
      await expect(page).toHaveURL(new RegExp(`/books/${id}/edit$`));

      await page.getByTestId("me-discard").click();
    });

    // ---------------------------------------------------------------------------
    // Fetch -> preview -> select all -> apply -> save writes through overrides
    // (AC1 preview, AC2 apply writes through and is revertible)
    // ---------------------------------------------------------------------------

    test("select-all applies every fetched field and saving writes an override that can be reverted", async ({
      page,
      request,
    }) => {
      const id = await fetchBookIdByTitle(request, TARGET.title);
      await mockAvailable(page, true);
      await gotoReady(page, `/books/${id}/edit`);

      await mockFetchResult(page, FOUND_RESULT);
      await clickFetch(page);

      await page.getByTestId("hardcover-fetch-select-all").click();
      await page.getByTestId("hardcover-fetch-apply").click();

      await expect(page.getByLabel("Title")).toHaveValue(
        "La Foret des Algorithmes: Deuxieme Edition",
      );
      await expect(page.locator("#me-description")).toHaveValue(
        "A revised survey of recursive structures in the deep woods.",
      );
      await expect(page.getByLabel("Series")).toHaveValue(
        "Mathematique Moderne",
      );
      await expect(page.getByLabel("Book #")).toHaveValue("2");
      await expect(page.getByLabel("ISBN-13")).toHaveValue("9781234567897");
      await expect(
        page.locator(".me-chip-item").getByText("Emmy Noether"),
      ).toBeVisible();

      // Save writes through the existing overrides RPC — the fetch/apply panel
      // never talks to it directly.
      await expectMutation(
        page,
        {
          method: "POST",
          url: /\/api\/rpc\/ebook\/overrides$/,
          expectedStatus: 200,
        },
        async () => page.getByTestId("me-save").click(),
      );

      await expect(page).toHaveURL(new RegExp(`/books/${id}$`));
      await expect(
        page.getByRole("heading", {
          level: 1,
          name: "La Foret des Algorithmes: Deuxieme Edition",
        }),
      ).toBeVisible();

      // Override is active and revertible from the edit form.
      await gotoReady(page, `/books/${id}/edit`);
      const revertBtn = page.getByTestId("revert-overrides");
      await expect(revertBtn).toBeVisible();

      await expectMutation(
        page,
        {
          method: "POST",
          url: /\/api\/rpc\/ebook\/overrides\/delete$/,
          expectedStatus: 200,
        },
        async () => revertBtn.click(),
      );

      await expect(page).toHaveURL(new RegExp(`/books/${id}$`));
      await expect(
        page.getByRole("heading", { level: 1, name: TARGET.title }),
      ).toBeVisible();
    });
  }); // test.describe.serial
