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

test("a linked book resolves silently on the player — no prompt, mapped seek", async ({
  page,
  request,
}) => {
  // Linking turns follow on, and follow-only is the whole surface now:
  // the old "jump ahead?" card is gone, the boot itself seeds the mapped
  // position (resolve_follow_boot). Reading ahead at 50% of the text maps
  // to ~1s of the 2-second fixture — the seek slider must boot past 0
  // with no prompt card anywhere.
  await writeAudioSeconds(request, 0);
  await writeEpubPercent(request, 50);

  await gotoReady(page, `/listen/${uuid}`);
  await page.waitForLoadState("networkidle");
  // "No prompt card": the declare pill is the only sync affordance, and
  // the removed offer card's user-facing copy ("Jump ahead?" / "Sync to")
  // appears nowhere — pinned to the historical strings so a resurrected
  // prompt would fail this, unlike a role/testid that never existed.
  await expect(page.getByTestId("listen-sync-here")).toBeVisible();
  await expect(page.getByText(/Jump ahead|Sync to \d/)).toHaveCount(0);
  const seek = page.getByRole("slider", { name: "Seek" });
  await expect
    .poll(async () => Number(await seek.inputValue()), {
      timeout: 15_000,
      message: "the boot should seed the mapped (non-zero) position",
    })
    .toBeGreaterThan(0);
});

test("the hero shows one synced card with the counterpart affordance", async ({
  page,
  request,
}) => {
  // The rail holds the five newest positions and drops finished books —
  // and the 2-second fixture MP3 finishes on any playback, which flips
  // read status to `finished`. Reset to `reading`, re-stamp, and reload
  // until the card lands (the established hero-spec pattern).
  //
  // Re-anchor the synthetic clock to wall time first (test 4's pattern):
  // the file-level clock starts at wall−60 and gains one second per
  // write, while parallel specs stamp wall-clock positions — without the
  // re-anchor this card can be out-ranked for the entire retry window.
  clock = Math.floor(Date.now() / 1000) - 3;
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

  // The linked dual-format card also carries the Immersive pill, which
  // opens the reader with the audiobook docked (same handler as the
  // book-detail CTA — immersive.spec.ts covers the dock itself).
  const immersive = page.getByTestId(`hero-immersive-${uuid}`);
  await expect(immersive).toBeVisible();
  await immersive.click();
  await expect(page).toHaveURL(new RegExp(`/read/${uuid}$`));
});

test("the book detail shows the newest-format progress row above the sync chip", async ({
  page,
}) => {
  // Linked and fresh: the marquee stage shows ONE position ruler for the book
  // plus the linked sync readout naming the audio-timeline hand-off.
  await gotoReady(page, `/books/${uuid}`);
  const row = page.getByTestId("sync-link-row");
  await expect(page.getByTestId("bdmq-ruler")).toBeVisible();
  await expect(page.getByTestId("sync-link-manage")).toBeVisible();
  // The one-spot line explains the mapped hand-off.
  await expect(row.getByText("one spot, both formats")).toBeVisible();
});

test("declaring a sync point turns on follow and the reader auto-applies", async ({
  page,
  request,
}) => {
  // Establish a REAL stored CFI first: the live failure mode was the
  // restore settle chain yanking the view back off the follow jump, and
  // that chain only engages when a stored CFI restore is in flight — a
  // percent-only row skips the restore entirely, which is how the earlier
  // version of this test missed it.
  await gotoReady(page, `/read/${uuid}`);
  await expectMutation(
    page,
    { method: "POST", url: /\/api\/rpc\/progress(?:\?|$)/ },
    async () => page.getByTestId("reader-next").click(),
  );

  // Earlier tests in this serial file leave WALL-clock progress writes
  // (a reader relocate, a player flush — and the page turn above), which
  // outrank this file's synthetic `now-60+n` clocks at the resume clock
  // gate — the follow candidate would read as NothingNewer and the
  // auto-apply would never fire. Re-anchor above the newest stored clock
  // (staying at-or-behind wall clock, which the server clamps to).
  let newest = 0;
  for (const format of ["epub", "audio"] as const) {
    const resp = await request.post("/api/rpc/progress/get", {
      data: { uuid, format },
    });
    if (resp.status() === 200) {
      const rec = (await resp.json()) as {
        client_updated_at: number | null;
      } | null;
      newest = Math.max(newest, rec?.client_updated_at ?? 0);
    }
  }
  // If the newest write is at (or near) wall clock, anchoring on it would
  // push nextClock() into the future, where the server clamps every write
  // to now — collapsing them onto one clock that then loses the
  // strictly-newer gate. Wait out the collision window instead so the
  // anchor is both strictly newer than every stored clock and far enough
  // behind wall clock for this test's three writes.
  await expect
    .poll(() => Math.floor(Date.now() / 1000), { timeout: 15_000 })
    .toBeGreaterThan(newest + 3);
  clock = Math.floor(Date.now() / 1000) - 3;

  // Reading behind (the stored page-turn CFI above), listening ahead —
  // then declare from the player so follow mode engages.
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
  // the mapped spot silently — no banner, and the view lands near the end.
  // Banner absence alone is not the contract (a silently-dropped jump also
  // shows no banner): the footer must show the moved position. The jump
  // fires before epub.js locations resolve, so this exercises the glue's
  // pending-jump path (displayPercentage parks the percentage and init's
  // locations hook applies it) — the fixture's `longBody` is what gives
  // the book enough locations for the jump to be visually distinct.
  await writeAudioSeconds(request, 2);
  await gotoReady(page, `/read/${uuid}`);
  await page.waitForLoadState("networkidle");
  // The footer's whole-book percent proves the view actually moved to the
  // mapped spot (audio at end → epub ≈ 100%), not merely that no banner
  // showed — a silently-dropped jump also shows no banner.
  await expect(page.getByTestId("reader-footer")).toContainText(
    /(8[6-9]|9\d|100)%/,
    { timeout: 20_000 },
  );
});
