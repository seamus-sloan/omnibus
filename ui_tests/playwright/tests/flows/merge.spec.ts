// F5.10 manual merge: admin "Merge with…" dialog on the book detail page.
//
// Serial mode: the happy-path test mutates shared server state (it merges
// two seeded books) and relies on its own Undo step as cleanup — the
// merged_uuids guard a merge leaves behind is keyed by path-stable uuids
// that survive re-seeding, so a leaked merge would silently re-attach on a
// later reindex and corrupt other specs' format counts. Never remove the
// undo step without re-thinking that.
import { expect, test } from "../fixtures/test";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { AUTO_ATTACHED_PAIRS } from "../fixtures/dual_format";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { withLock } from "../utils/lock";
import { expectNavVisible, gotoReady } from "../utils/nav";
import {
  audiobookFixturesDir,
  fixturesDir,
  seedAudiobookLibrary,
  seedLibrary,
} from "../utils/seed";

test.describe.configure({ mode: "serial" });

// Both libraries seeded: the merge pairs an EPUB-only ebook with an
// audiobook. Exactly one fixture pair ("Immersive Voyage") shares a
// normalized (title, author) and auto-attaches into a single dual-format
// book, so the combined total is the additive sum minus AUTO_ATTACHED_PAIRS.
// The merge TARGET/SOURCE below are chosen to avoid that pair.
test.beforeAll(async ({ request }) => {
  // Two sequential seed polls (ebooks then audiobooks), up to 45s each, so
  // this hook needs more than the 60s config default.
  test.setTimeout(120_000);
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT - AUTO_ATTACHED_PAIRS,
  );
});

// TARGET: EPUB-only standalone ("Alpha", Ada Lovelace). SOURCE: an MP3
// audiobook by the same author with a different title — same-author makes
// it a realistic merge, different-title keeps auto-attach out of the way.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;
const SOURCE = AUDIOBOOK_BOOKS.find((b) => b.author === "Ada Lovelace")!;

// Open the dialog, search for SOURCE, and select it — shared by the happy
// and error action tests.
async function openDialogAndPickSource(page: import("@playwright/test").Page) {
  await page.getByTestId("merge-with").click();
  await page.getByTestId("merge-search").fill(SOURCE.title);
  // The candidate list fills after the debounced search RPC returns —
  // wait for it explicitly so the click can't race the render.
  await expect(page.getByTestId("merge-candidate").first()).toBeVisible();
  await page.getByTestId("merge-candidate").first().click();
  await expect(page.getByTestId("merge-confirm")).toBeVisible();
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

test("renders the merge affordance on the book detail layout", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);
  await expectNavVisible(page);

  // Admin storageState: the rail shows "Merge with…" next to the
  // existing edit affordance. Clicking opens the dialog with its search
  // input; Cancel closes it without any mutation.
  await expect(page.getByTestId("edit-metadata")).toBeVisible();
  const mergeWith = page.getByTestId("merge-with");
  await expect(mergeWith).toBeVisible();
  await mergeWith.click();
  await expect(page.getByRole("dialog", { name: "Merge with another book" })).toBeVisible();
  await expect(page.getByTestId("merge-search")).toBeVisible();
  await page.getByTestId("merge-cancel").click();
  await expect(page.getByTestId("merge-search")).not.toBeVisible();
});

// ---------------------------------------------------------------------------
// Action — error path first (state must be untouched for the happy path)
// ---------------------------------------------------------------------------

test("surfaces a server error in the dialog and leaves the book unchanged", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.route("**/api/rpc/merge-books", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "merge exploded" }),
  );

  await openDialogAndPickSource(page);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/merge-books", expectedStatus: 500 },
    async () => page.getByTestId("merge-confirm").click(),
  );

  // The dialog stays open with an alert; the book keeps its single badge.
  await expect(page.getByRole("alert")).toBeVisible();
  await page.getByTestId("merge-cancel").click();
  await expect(page.getByTestId("bd-format-badge")).toHaveCount(1);
  await expect(page.getByTestId("bd-format-badge")).toHaveText("EPUB");
});

// ---------------------------------------------------------------------------
// Action — merge then undo (undo doubles as cleanup; see header comment)
// ---------------------------------------------------------------------------

test("merges an audiobook into the ebook and undoes it from the toast", async ({
  page,
  request,
}) => {
  // Serialize against book_detail.spec's file-picker test: both mutate the
  // same audiobook ("The Analytical Audiobook") on the shared per-shard server,
  // so parallel workers would corrupt each other's merge state. The lock spans
  // merge→undo so the fixture is restored before release. See utils/lock.ts.
  test.slow();
  await withLock("audiobook-merge", async () => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await gotoReady(page, `/books/${uuid}`);

    await openDialogAndPickSource(page);
    const { response } = await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/merge-books", expectedStatus: 200 },
      async () => page.getByTestId("merge-confirm").click(),
    );
    expect(response.status()).toBe(200);

    // Refetch lands: both format badges, both CTAs, and the undo toast.
    await expect(page.getByTestId("bd-format-badge")).toHaveCount(2);
    await expect(page.getByTestId("bd-format-badge").filter({ hasText: SOURCE.format })).toBeVisible();
    await expect(page.getByTestId("start-reading")).toBeVisible();
    await expect(page.getByTestId("listen-secondary")).toBeVisible();
    const toast = page.getByRole("status").filter({ hasText: "Books merged." });
    await expect(toast).toBeVisible();

    // Undo from the toast restores the split (and cleans up shared state).
    await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/merge-books/undo", expectedStatus: 200 },
      async () => page.getByTestId("merge-undo").click(),
    );
    await expect(page.getByTestId("bd-format-badge")).toHaveCount(1);
    await expect(page.getByTestId("bd-format-badge")).toHaveText("EPUB");
    await expect(page.getByTestId("listen-secondary")).not.toBeVisible();
  });
});
