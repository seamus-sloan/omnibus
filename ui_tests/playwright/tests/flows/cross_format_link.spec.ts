// Cross-format sync link — the alignment modal and the book-detail entry
// row (unlinked nudge → linked chip → unlink), for a book carrying BOTH
// formats. Uses the "Immersive Voyage" auto-attached pair (the one
// intentional dual-format fixture; see `fixtures/dual_format.ts`).
//
// Serial: a confirmed `cross_format_links` row is per-(user, book) server
// state visible to every worker, so the spec owns its writes and restores
// the unlinked state on the way out.

import type { APIRequestContext } from "@playwright/test";
import { AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { AUTO_ATTACHED_PAIRS, DUAL_FORMAT_BOOK } from "../fixtures/dual_format";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { gotoReady } from "../utils/nav";
import {
  audiobookFixturesDir,
  fixturesDir,
  seedAudiobookLibrary,
  seedLibrary,
} from "../utils/seed";

test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ request }) => {
  // Two sequential seed polls (ebooks then audiobooks), up to 45s each.
  test.setTimeout(120_000);
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT - AUTO_ATTACHED_PAIRS,
  );
});

/** Poll until the fixture book reports both formats, then return its uuid. */
async function fetchDualFormatUuid(
  request: APIRequestContext,
): Promise<string> {
  const isEbook = (f: string) => /^(epub|pdf)$/i.test(f);
  const isAudio = (f: string) => /^(mp3|m4b)$/i.test(f);
  let uuid: string | null = null;
  await expect
    .poll(
      async () => {
        const resp = await request.get("/api/rpc/ebooks");
        if (resp.status() !== 200) return false;
        const body = (await resp.json()) as {
          books: {
            title: string | null;
            unique_identifier: string | null;
            formats: string[];
          }[];
        };
        const match = body.books.find(
          (b) => b.title === DUAL_FORMAT_BOOK.title,
        );
        if (!match?.formats.some(isEbook) || !match.formats.some(isAudio)) {
          return false;
        }
        uuid = match.unique_identifier;
        return uuid !== null;
      },
      { timeout: 45_000 },
    )
    .toBe(true);
  // The poll above only succeeds once `uuid` is non-null.
  return uuid!;
}

test("renders the sync entry row for a dual-format book", async ({
  page,
  request,
}) => {
  const uuid = await fetchDualFormatUuid(request);
  await gotoReady(page, `/books/${uuid}`);

  const row = page.getByTestId("sync-link-row");
  await expect(row).toBeVisible();
  // Unlinked by default: the nudge and its open affordance.
  await expect(page.getByTestId("sync-link-open")).toBeVisible();
  await expect(
    page.getByText("Positions aren't synced", { exact: false }),
  ).toBeVisible();
});

test("links the formats, shows the chip, then unlinks", async ({
  page,
  request,
}) => {
  const uuid = await fetchDualFormatUuid(request);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("sync-link-open").click();
  const modal = page.getByTestId("alignment-modal");
  await expect(modal).toBeVisible();
  // The dual-format fixture is a single audio file, so the simple variant
  // renders: lanes, no sequence/narrations choice.
  await expect(page.getByTestId("alignment-lanes")).toBeVisible();
  await expect(page.getByTestId("alignment-choice")).toHaveCount(0);
  // The fixture's lone audio mark can't pair with the ebook's chapters, so
  // the mismatch (percentage) state renders: warn banner, ~ pill, no
  // footer note, and a confirm that names the mapping. The pill's counts
  // vary with the structure-backfill race, so assert only the stable tail.
  await expect(page.getByTestId("alignment-lowconf")).toContainText(
    "so sync will go by percentage instead",
  );
  await expect(page.getByTestId("alignment-linear-pill")).toContainText(
    "no 1:1 match",
  );
  await expect(page.getByText("Sync stays off until you confirm")).toHaveCount(
    0,
  );

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cross-format/link",
      expectedStatus: 200,
    },
    async () =>
      modal.getByRole("button", { name: "Sync Based Off Percentage" }).click(),
  );
  await expect(modal).toHaveCount(0);
  await expect(page.getByTestId("sync-link-manage")).toBeVisible();

  // Restore the unlinked state so no other worker inherits the link.
  await page.getByTestId("sync-link-manage").click();
  await expect(page.getByTestId("alignment-modal")).toBeVisible();
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cross-format/unlink",
      expectedStatus: 200,
    },
    async () => page.getByTestId("alignment-unlink").click(),
  );
  await expect(page.getByTestId("sync-link-open")).toBeVisible();
});

test("surfaces a failed link confirm and keeps sync off", async ({
  page,
  request,
}) => {
  const uuid = await fetchDualFormatUuid(request);
  await gotoReady(page, `/books/${uuid}`);

  await page.route("**/api/rpc/cross-format/link", (route) =>
    route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "forced failure",
    }),
  );

  await page.getByTestId("sync-link-open").click();
  await expect(page.getByTestId("alignment-modal")).toBeVisible();
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cross-format/link",
      expectedStatus: 500,
    },
    async () =>
      page.getByRole("button", { name: "Sync Based Off Percentage" }).click(),
  );
  // The modal stays up with the error surfaced; sync stayed off.
  await expect(page.getByRole("alert")).toBeVisible();
  await page.unroute("**/api/rpc/cross-format/link");
  await page.getByTestId("alignment-cancel").click();
  await expect(page.getByTestId("sync-link-open")).toBeVisible();
});
