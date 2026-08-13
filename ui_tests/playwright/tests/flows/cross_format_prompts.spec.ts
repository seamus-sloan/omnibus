// Cross-format resume prompts + the unified Continue hero, on the
// RESERVED "Parallel Latitudes" dual pair (see fixtures/dual_format.ts):
// this spec confirms a link and writes reading/listening progress — all
// globally visible server state — so no other spec may touch the book.
//
// Serial: the tests build on one linked book and strictly increasing
// position clocks.

import type { APIRequestContext } from "@playwright/test";
import { AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import {
  AUTO_ATTACHED_PAIRS,
  RESUME_PROMPT_BOOK,
} from "../fixtures/dual_format";
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

// Strictly increasing event clocks, anchored to real time (the server
// clamps future clocks to now, so stay at-or-behind wall clock).
let clock = Math.floor(Date.now() / 1000) - 60;
const nextClock = () => ++clock;

let uuid = "";

test.beforeAll(async ({ request }) => {
  test.setTimeout(120_000);
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT - AUTO_ATTACHED_PAIRS,
  );
  uuid = await fetchDualUuid(request);
  const link = await request.post("/api/rpc/cross-format/link", {
    data: { update: { book_uuid: uuid, mode: "sequence" } },
  });
  expect(link.status(), "link confirm failed").toBe(200);
});

test.afterAll(async ({ request }) => {
  // Restore the unlinked state so a future spec inherits nothing.
  if (uuid) {
    await request.post("/api/rpc/cross-format/unlink", {
      data: { uuid },
    });
  }
});

async function fetchDualUuid(request: APIRequestContext): Promise<string> {
  const isEbook = (f: string) => /^(epub|pdf)$/i.test(f);
  const isAudio = (f: string) => /^(mp3|m4b)$/i.test(f);
  let found: string | null = null;
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
          (b) => b.title === RESUME_PROMPT_BOOK.title,
        );
        if (!match?.formats.some(isEbook) || !match.formats.some(isAudio)) {
          return false;
        }
        found = match.unique_identifier;
        return found !== null;
      },
      { timeout: 45_000 },
    )
    .toBe(true);
  return found!;
}

async function writeEpubPercent(
  request: APIRequestContext,
  percent: number,
): Promise<void> {
  const resp = await request.post("/api/rpc/progress", {
    data: {
      update: {
        book_uuid: uuid,
        format: "epub",
        progress_percent: percent,
        client_updated_at: nextClock(),
      },
    },
  });
  expect(resp.status(), "epub progress write failed").toBe(200);
}

async function writeAudioSeconds(
  request: APIRequestContext,
  seconds: number,
): Promise<void> {
  const resp = await request.post("/api/rpc/progress", {
    data: {
      update: {
        book_uuid: uuid,
        format: "audio",
        audio_position_seconds: seconds,
        client_updated_at: nextClock(),
      },
    },
  });
  expect(resp.status(), "audio progress write failed").toBe(200);
}

test("the player offers the mapped jump when reading is ahead", async ({
  page,
  request,
}) => {
  // Audio parked at the start: on the 2-second fixture MP3, any later
  // position would sit at/past the mapped reading spot and read as a
  // backward offer under the equivalence gate's direction flag.
  await writeAudioSeconds(request, 0);
  await writeEpubPercent(request, 50);

  await gotoReady(page, `/listen/${uuid}`);
  const prompt = page.getByTestId("sync-prompt");
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText("jump to");

  await page.getByTestId("sync-prompt-accept").click();
  await expect(prompt).toHaveCount(0);
});

test("declining stores the spot and re-arms only when reading advances", async ({
  page,
  request,
}) => {
  await writeEpubPercent(request, 60);
  await gotoReady(page, `/listen/${uuid}`);
  await expect(page.getByTestId("sync-prompt")).toBeVisible();

  await page.getByTestId("sync-prompt-dismiss").click();
  await expect(page.getByTestId("sync-prompt")).toHaveCount(0);

  // Same position after a fresh navigation: still quiet. Wait for the
  // network to settle so the candidate fetch has definitely resolved
  // before asserting absence.
  await gotoReady(page, `/listen/${uuid}`);
  await page.waitForLoadState("networkidle");
  await expect(page.getByTestId("sync-prompt")).toHaveCount(0);

  // The reading position advances: the prompt re-arms.
  await writeEpubPercent(request, 70);
  await gotoReady(page, `/listen/${uuid}`);
  await expect(page.getByTestId("sync-prompt")).toBeVisible();
  await page.getByTestId("sync-prompt-dismiss").click();
});

test("the reader offers the inverse jump when listening is ahead", async ({
  page,
  request,
}) => {
  await writeAudioSeconds(request, 55);

  await gotoReady(page, `/read/${uuid}`);
  const banner = page.getByTestId("sync-banner");
  await expect(banner).toBeVisible();
  await expect(banner).toContainText("%");

  await page.getByTestId("sync-banner-dismiss").click();
  await expect(banner).toHaveCount(0);
});

test("the hero shows one synced card with the counterpart affordance", async ({
  page,
  request,
}) => {
  // The rail holds the five newest positions and drops finished books —
  // and the 2-second fixture MP3 finishes on any playback, which flips
  // read status to `finished`. Reset to `reading`, re-stamp, and reload
  // until the card lands (the established hero-spec pattern).
  await expect
    .poll(
      async () => {
        const status = await request.post("/api/rpc/read-status/set", {
          data: { update: { book_uuid: uuid, status: "reading" } },
        });
        expect(status.status(), "read-status reset failed").toBe(200);
        await writeEpubPercent(request, 80);
        await gotoReady(page, "/");
        return page
          .getByTestId(`hero-card-${uuid}`)
          .waitFor({ state: "visible", timeout: 3_000 })
          .then(() => true)
          .catch(() => false);
      },
      { timeout: 45_000, intervals: [1_000] },
    )
    .toBe(true);

  const cards = page.getByTestId(`hero-card-${uuid}`);
  await expect(cards).toHaveCount(1);
  await expect(cards).toContainText("synced");
  await expect(page.getByTestId(`hero-crossformat-${uuid}`)).toBeVisible();
});

test("declaring a sync point turns on follow and the reader auto-applies", async ({
  page,
  request,
}) => {
  // Fresh positions: reading behind, listening ahead — then declare from
  // the player so follow mode engages.
  await writeEpubPercent(request, 85);
  await writeAudioSeconds(request, 1);

  await gotoReady(page, `/listen/${uuid}`);
  await expectMutation(
    page,
    {
      method: "POST",
      url: `/api/rpc/cross-format/sync-point`,
      expectedStatus: 200,
    },
    async () => page.getByTestId("listen-sync-here").click(),
  );
  await expect(page.getByTestId("listen-sync-here")).toContainText("Synced");

  // Listening advances past the declared pair; the reader then opens at
  // the mapped spot silently — no banner, and the relocation writes the
  // followed position into the epub row.
  await writeAudioSeconds(request, 2);
  await gotoReady(page, `/read/${uuid}`);
  await page.waitForLoadState("networkidle");
  await expect(page.getByTestId("sync-banner")).toHaveCount(0);
});
