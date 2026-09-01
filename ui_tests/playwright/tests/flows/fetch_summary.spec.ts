import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The "Fetch Summary" button on the **book detail page** pulls a blurb from
// the configured source cascade (Hardcover → Google Books → Open Library) on
// demand. The generated fixtures ship no descriptions, so every fixture book
// is "sparse" (< 10 words) — the condition under which the button appears.
// The source-plan and fetch endpoints are mocked via `page.route` so CI never
// touches a live external API.
//
// The metadata editor no longer carries this button: "Fetch metadata" (see
// `metadata_edit_search.spec.ts`) is the one fetch-from-outside action on
// that page, and it fills the description among everything else.

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  // Clear any description override on the target so it starts sparse (the
  // detail button's precondition) and its editor field starts empty —
  // hermetic regardless of overrides a prior run/manual test may have left.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await request.delete(`/api/ebooks/${uuid}/overrides`);
});

const SOURCES_URL = "**/api/rpc/ebook/summary/sources";
const FETCH_URL = "**/api/rpc/ebook/summary/fetch";

/** Mock the ordered source plan the client walks (e.g. `["OpenLibrary"]`). */
async function mockSources(
  page: Parameters<typeof gotoReady>[0],
  sources: string[],
) {
  await page.route(SOURCES_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(sources),
        })
      : route.continue(),
  );
}

/** Mock the summary fetch. `text` → `Ok(Some(text))`; `null` → `Ok(None)` (a miss). */
async function mockFetch(
  page: Parameters<typeof gotoReady>[0],
  text: string | null,
) {
  await page.route(FETCH_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(text),
        })
      : route.continue(),
  );
}

test("detail page fetch saves the summary and hides the button", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Sparse (empty) summary → the button is offered.
  await expect(page.getByTestId("fetch-summary")).toBeVisible();

  // 14 words, so once saved the summary is no longer sparse and the button hides.
  const SUMMARY =
    "A sufficiently long mocked summary with more than ten words to enrich the book.";
  await mockSources(page, ["OpenLibrary"]);
  await mockFetch(page, SUMMARY);
  // Mock the override save so no real DB write leaks across specs; `null` decodes
  // as `Ok(None)`, which the client treats as a successful save.
  await page.route("**/api/rpc/ebook/overrides", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: "null",
        })
      : route.continue(),
  );

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/ebook/overrides",
      expectedBody: { uuid, overrides: { description: SUMMARY } },
      expectedStatus: 200,
    },
    async () => page.getByTestId("fetch-summary").click(),
  );

  await expect(page.getByTestId("book-description")).toContainText(SUMMARY);
  await expect(page.getByTestId("fetch-summary")).toBeHidden();
});

test("expands a clamped description and collapses it again", async ({
  page,
  request,
}) => {
  // `.bdmq-desc` clamps to five lines so a long blurb can't push the CTA row
  // off the stop; before #2290 there was no way to read the rest. The save is
  // mocked (as above) so nothing leaks into the shared library.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const LONG = `${"Every clause here exists only to push this blurb past the clamp. ".repeat(8)}End.`;
  await mockSources(page, ["OpenLibrary"]);
  await mockFetch(page, LONG);
  await page.route("**/api/rpc/ebook/overrides", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: "null",
        })
      : route.continue(),
  );
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/ebook/overrides", expectedStatus: 200 },
    async () => page.getByTestId("fetch-summary").click(),
  );

  const desc = page.getByTestId("book-description");
  const toggle = page.getByTestId("book-description-toggle");
  await expect(toggle).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(desc).not.toHaveClass(/is-open/);

  // Keyboard-reachable, per AC2 — focus it and press Enter rather than click.
  await toggle.focus();
  await expect(toggle).toBeFocused();
  await toggle.press("Enter");

  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(desc).toHaveClass(/is-open/);
  // The whole blurb is now rendered, not just the clamped five lines.
  await expect
    .poll(async () =>
      desc.evaluate((el) => el.scrollHeight - el.clientHeight <= 1),
    )
    .toBe(true);

  await toggle.press("Enter");
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(desc).not.toHaveClass(/is-open/);
});

test("offers no expand control for a description that fits", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const SHORT =
    "A sufficiently long mocked summary with more than ten words to enrich the book.";
  await mockSources(page, ["OpenLibrary"]);
  await mockFetch(page, SHORT);
  await page.route("**/api/rpc/ebook/overrides", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: "null",
        })
      : route.continue(),
  );
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/ebook/overrides", expectedStatus: 200 },
    async () => page.getByTestId("fetch-summary").click(),
  );

  await expect(page.getByTestId("book-description")).toContainText(SHORT);
  await expect(page.getByTestId("book-description-toggle")).toHaveCount(0);
});
