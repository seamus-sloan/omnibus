import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Journals are public + global per book: every test here lands on the same
// `alpha` book and mutates the shared per-book feed, so run serially (mirrors
// book_detail.spec.ts) — `playwright.config.ts` is otherwise `fullyParallel`,
// which would interleave these and let one test's entries race another's
// assertions. Each test still creates a uniquely-marked entry and deletes it.
test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

/** Open the inline composer and publish `body`, asserting the create POST. */
async function publish(page: import("@playwright/test").Page, body: string) {
  await page.getByTestId("journal-open-composer").click();
  await page.getByTestId("journal-body").fill(body);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/create", expectedStatus: 200 },
    async () => page.getByTestId("journal-publish").click(),
  );
}

/** Delete the entry whose rendered body contains `marker`. */
async function deleteEntry(page: import("@playwright/test").Page, marker: string) {
  const card = page.getByTestId("journal-entry").filter({ hasText: marker });
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/delete", expectedStatus: 200 },
    async () => card.getByTestId("journal-delete").click(),
  );
  await expect(page.getByTestId("journal-entry").filter({ hasText: marker })).toHaveCount(0);
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

test("renders the journal section layout", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await expectNavVisible(page);

  // Section heading + shared-log blurb + collapsed composer prompt.
  await expect(page.getByTestId("journal-section")).toBeVisible();
  await expect(page.getByRole("heading", { name: "What readers have written" })).toBeVisible();
  await expect(page.getByTestId("journal-open-composer")).toBeVisible();

  // Expanding the composer reveals the body field and the spoiler-syntax hint.
  await page.getByTestId("journal-open-composer").click();
  await expect(page.getByTestId("journal-composer")).toBeVisible();
  await expect(page.getByTestId("journal-body")).toBeVisible();
  await expect(page.getByTestId("journal-spoiler-help")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Action — create, edit, delete (happy path)
// ---------------------------------------------------------------------------

test("publishes an entry attributed to the current user, edits it, then deletes it", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const marker = `e2e-journal-${Date.now()}`;
  await publish(page, `First thoughts ${marker}`);

  // The new entry renders with the author's "you" chip (current user owns it).
  const card = page.getByTestId("journal-entry").filter({ hasText: marker });
  await expect(card).toBeVisible();
  await expect(card.getByText("you")).toBeVisible();

  // Owner edit → the body updates in place. Entering edit mode swaps the
  // rendered body for a textarea, so the marker-filtered `card` stops matching
  // (a textarea's value isn't DOM text) — target the single open editor at page
  // level instead.
  await card.getByTestId("journal-edit").click();
  await page.getByTestId("journal-edit-body").fill(`Revised thoughts ${marker}`);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/update", expectedStatus: 200 },
    async () => page.getByTestId("journal-edit-save").click(),
  );
  await expect(
    page.getByTestId("journal-entry").filter({ hasText: `Revised thoughts ${marker}` }),
  ).toBeVisible();

  await deleteEntry(page, marker);
});

// ---------------------------------------------------------------------------
// Action — markdown preview + spoiler reveal
// ---------------------------------------------------------------------------

test("renders a markdown preview and blurs spoilers until clicked", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Preview renders server-sanitized markdown for the draft.
  await page.getByTestId("journal-open-composer").click();
  await page.getByTestId("journal-body").fill("**bold preview**");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/preview", expectedStatus: 200 },
    async () => page.getByTestId("journal-preview-toggle").click(),
  );
  await expect(page.getByTestId("journal-preview").locator("strong")).toHaveText("bold preview");

  // Publish an entry containing a spoiler; it renders blurred until clicked.
  const marker = `e2e-spoiler-${Date.now()}`;
  // Switch back to the Write tab (Preview hides the textarea) and compose.
  await page.getByText("Write", { exact: true }).click();
  await page.getByTestId("journal-body").fill(`reveal ${marker}: ||the secret||`);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/create", expectedStatus: 200 },
    async () => page.getByTestId("journal-publish").click(),
  );

  const spoiler = page
    .getByTestId("journal-entry")
    .filter({ hasText: marker })
    .locator(".spoiler");
  await expect(spoiler).toHaveText("the secret");
  await expect(spoiler).not.toHaveClass(/revealed/);
  await spoiler.click();
  await expect(spoiler).toHaveClass(/revealed/);

  await deleteEntry(page, marker);
});

// ---------------------------------------------------------------------------
// Error path — failed publish surfaces an error, composer stays open
// ---------------------------------------------------------------------------

test("surfaces an error and keeps the draft when publishing fails", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.route("**/api/rpc/journals/create", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "journal exploded" }),
  );

  await page.getByTestId("journal-open-composer").click();
  await page.getByTestId("journal-body").fill("this will fail");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/create", expectedStatus: 500 },
    async () => page.getByTestId("journal-publish").click(),
  );

  // The composer stays open with the draft intact and an inline error.
  await expect(page.getByTestId("journal-composer")).toBeVisible();
  await expect(page.getByTestId("journal-body")).toHaveValue("this will fail");
  await expect(page.getByTestId("journal-composer").getByRole("alert")).toBeVisible();
});
