import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The chart builder is a standalone surface while `/stats` is being
// redesigned, so nothing in the UI links to it yet and this spec reaches it by
// URL. That is a deliberate departure from the navigation rule in
// `.claude/rules/04-playwright.md` — recorded here rather than left implicit,
// and it lapses the moment the builder gets its entry point on `/stats`.
const CHART = "/stats/chart";

// Every assertion below is about the picker's own contract, never about
// plotted values. Reading history is global shared state — the reader and
// read-status specs write it — so a spec that asserted a bar's height would be
// asserting whatever the rest of the suite happened to have done first.

// Control lookups are scoped to the picker: the plot's own accessible name
// ends "over N periods", which an unscoped `getByLabel("Period")` matches
// alongside the select.
const control = (page: Page, label: string) =>
  page.getByTestId("chart-controls").getByLabel(label);

// ── A stubbed chart ─────────────────────────────────────────────────────
//
// The series a chart draws come from the caller's own reading history, and
// this suite has none it can rely on: it runs as the shared test user against
// a database whose reading rows are whatever other specs happened to write.
// Asserting on marks under those conditions passes locally, where a developer
// database has accumulated sittings, and fails on a fresh CI one — which is
// exactly what happened.
//
// So the wire is stubbed instead. That draws the line where it belongs: what
// the *server* computes is covered by `db::stats::builder`'s tests, and what
// the *page* does with a well-formed result is covered here, deterministically.
// The stub echoes the spec it was posted so the page's own reactions — a
// second scale appearing, a split stacking, a regrouping replacing the axis —
// are still driven by the picker rather than hard-coded.

const UNIT: Record<string, string> = {
  books_finished: "books",
  avg_page_length: "pages",
  pages_read: "pages",
  avg_rating: "stars",
  reading_minutes: "minutes",
  listening_minutes: "minutes",
  avg_session_minutes: "minutes",
  session_count: "sessions",
};
// Totals are bars, means are lines — mirroring `ChartMeasure::mark`.
const IS_BAR: Record<string, boolean> = {
  books_finished: true,
  session_count: true,
  reading_minutes: true,
  listening_minutes: true,
  pages_read: true,
  avg_page_length: false,
  avg_rating: false,
  avg_session_minutes: false,
};
const STUB_GENRES = ["Fantasy", "Horror", "Crime"];

function stubBuckets(bucket: string): string[] {
  if (bucket === "year") return ["2024", "2025", "2026"];
  if (bucket === "day")
    return Array.from({ length: 8 }, (_, i) => `2026-08-0${i + 1}`);
  if (bucket === "week")
    return ["2026-06-01", "2026-06-08", "2026-06-15", "2026-06-22"];
  return Array.from({ length: 8 }, (_, i) => `2026-0${i + 1}`);
}

function stubResult(spec: {
  measures: string[];
  bucket: string;
  breakdown: string;
}): unknown {
  const buckets = stubBuckets(spec.bucket);
  const value = (i: number, seed: number) => 1 + ((i + seed) % 4);

  if (spec.breakdown === "genre" && spec.measures.length === 1) {
    const m = spec.measures[0]!;
    return {
      bucket: spec.bucket,
      buckets,
      series: STUB_GENRES.map((g, gi) => ({
        measure: m,
        slice: g,
        axis: 0,
        mark: IS_BAR[m] ? "bar" : "line",
        values: buckets.map((_, i) => value(i, gi)),
      })),
      axes: [{ unit: UNIT[m], max: 20 }],
      divisions: 4,
      // Slices of an additive measure are parts of a whole; means never stack.
      stacked: IS_BAR[m] ?? false,
      truncated: false,
      caveats: [],
    };
  }

  const units: string[] = [];
  for (const m of spec.measures) {
    const u = UNIT[m]!;
    if (!units.includes(u)) units.push(u);
  }
  return {
    bucket: spec.bucket,
    buckets,
    series: spec.measures.map((m, si) => ({
      measure: m,
      slice: null,
      axis: units.indexOf(UNIT[m]!),
      mark: IS_BAR[m] ? "bar" : "line",
      values: buckets.map((_, i) => value(i, si)),
    })),
    axes: units.map((u) => ({ unit: u, max: 20 })),
    divisions: 4,
    stacked: false,
    truncated: false,
    // `pages_read` is the one measure whose coverage is bounded, and the
    // server sends its caveat with the result.
    caveats: spec.measures.includes("pages_read")
      ? [
          "Pages read is measured from the progress ledger, which only covers reading recorded after it was introduced. Earlier buckets read low.",
        ]
      : [],
  };
}

async function stubChart(page: Page): Promise<void> {
  await page.route("**/api/rpc/chart-series", async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    const posted = JSON.parse(route.request().postData() ?? "{}");
    // The server function takes the spec as its single argument; accept either
    // the bare object or a one-key wrapper so this does not hinge on the
    // encoding's shape.
    const spec = posted.spec ?? posted;
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(stubResult(spec)),
    });
  });
}

// The builder opens with nothing plotted, so any test that needs a chart
// stubs the wire and selects one. This pair — a count against an average, on
// two scales — is the comparison the page exists for.
async function plotThePair(page: Page): Promise<void> {
  await stubChart(page);
  await control(page, "Books finished").check();
  await control(page, "Avg book length").check();
  await expect
    .poll(async () => page.getByTestId("chart-legend").count())
    .toBeGreaterThan(0);
}

// Drag across the plot from band `a` to band `b`. The plot sits below the
// picker in a 720-high viewport, so it is scrolled into view first — a drag
// aimed at a band's centre would otherwise land off-screen.
async function brush(page: Page, a: number, b: number): Promise<void> {
  await page.locator(".cb-svg").scrollIntoViewIfNeeded();
  const bands = page.locator(".cb-hit");
  const from = await bands.nth(a).boundingBox();
  const to = await bands.nth(b).boundingBox();
  if (!from || !to) throw new Error("no band geometry to brush across");
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, {
    steps: 10,
  });
  await page.mouse.up();
}

test("renders the chart builder layout", async ({ page }) => {
  await gotoReady(page, CHART);

  await expectNavVisible(page);
  await expect(
    page.getByRole("heading", { name: "Chart builder" }),
  ).toBeVisible();
  await expect(page.getByTestId("chart-controls")).toBeVisible();
  await expect(page.getByTestId("chart-measures")).toBeVisible();
  // Opens empty, so the prompt stands where the chart will be.
  await expect(page.getByTestId("chart-prompt")).toBeVisible();
  await expect(page.getByTestId("chart-canvas")).toHaveCount(0);

  for (const label of ["Group by", "Period", "Split"]) {
    await expect(control(page, label)).toBeVisible();
  }
});

test("opens with nothing plotted and nothing greyed out", async ({ page }) => {
  await gotoReady(page, CHART);

  // No opinion pre-loaded, and — because no unit has claimed a scale — the
  // whole vocabulary is available, so the compatibility rule is visible
  // rather than discovered by being blocked.
  for (const name of ["Books finished", "Avg rating", "Reading minutes"]) {
    await expect(control(page, name)).not.toBeChecked();
    await expect(control(page, name)).toBeEnabled();
  }
  await expect(page.getByTestId("chart-prompt")).toBeVisible();

  // Ticking one draws it.
  await control(page, "Books finished").check();
  await expect(page.getByTestId("chart-prompt")).toHaveCount(0);
  await expect(page.getByTestId("chart-canvas")).toBeVisible();
});

test("lets the last measure be cleared back to an empty chart", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  await control(page, "Books finished").check();
  await expect(page.getByTestId("chart-canvas")).toBeVisible();

  // Clearing the chart is a state a reader is allowed to reach.
  await control(page, "Books finished").uncheck();
  await expect(page.getByTestId("chart-prompt")).toBeVisible();
  await expect(page.getByTestId("chart-canvas")).toHaveCount(0);
});

test("offers every measure with the unit that decides its compatibility", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  const measures = page.getByTestId("chart-measures");

  for (const name of [
    "Books finished",
    "Avg book length",
    "Avg rating",
    "Reading minutes",
    "Listening minutes",
    "Sittings",
    "Avg sitting length",
    "Pages read",
  ]) {
    await expect(measures.getByLabel(name)).toBeVisible();
  }
  await expect(measures).toContainText("books · per book finished");
  await expect(measures).toContainText("minutes · per sitting");
});

test("narrows the list only once both scales are claimed", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);

  // Books + pages, so both scales are in use: a third unit is blocked, but a
  // second pages measure still joins the scale pages already own.
  await expect(page.getByTestId("chart-scales")).toContainText(
    "Both scales in use",
  );
  await expect(control(page, "Avg rating")).toBeDisabled();
  await expect(control(page, "Reading minutes")).toBeDisabled();
  await expect(control(page, "Pages read")).toBeEnabled();

  // Freeing the pages scale reopens every unit.
  await control(page, "Avg book length").uncheck();
  await expect(page.getByTestId("chart-scales")).toContainText(
    "anything else can still join",
  );
  await expect(control(page, "Avg rating")).toBeEnabled();
  await expect(control(page, "Reading minutes")).toBeEnabled();
});

test("plots more measures than there are scales", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);

  // A third measure sharing an existing unit is allowed — the bound is on
  // scales, not on how many measures use them.
  await control(page, "Pages read").check();
  await expect(control(page, "Pages read")).toBeChecked();

  await expect
    .poll(async () => page.getByTestId("chart-legend").count())
    .toBeGreaterThan(0);
  const legend = page.getByTestId("chart-legend");
  await expect(legend).toContainText("Books finished");
  await expect(legend).toContainText("Avg book length");
  await expect(legend).toContainText("Pages read");
});

test("offers a split only for a single per-book measure", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);
  const split = control(page, "Split");

  // Two measures, so no single population for a split to describe.
  await expect(split).toBeDisabled();
  await expect(page.getByTestId("chart-controls")).toContainText(
    "one measure only",
  );

  await control(page, "Avg book length").uncheck();
  await expect(split).toBeEnabled();

  // A sitting cannot carry a genre — a sitting may cover several books, and a
  // book several genres, so splitting one would double-count its minutes.
  await control(page, "Books finished").check();
  await control(page, "Reading minutes").check();
  await control(page, "Books finished").uncheck();
  await expect(split).toBeDisabled();
  await expect(page.getByTestId("chart-controls")).toContainText(
    "only per-book measures split",
  );
});

test("redraws when the selection changes", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);
  const canvas = page.getByTestId("chart-canvas");

  // Either a chart or the empty state, depending on what reading history the
  // rest of the suite has produced — but the canvas always resolves to one of
  // them rather than hanging.
  const settled = async () =>
    expect
      .poll(async () =>
        (await page.getByTestId("chart-empty").count()) > 0
          ? "empty"
          : (await page.getByTestId("chart-legend").count()) > 0
            ? "chart"
            : "pending",
      )
      .not.toBe("pending");

  await settled();

  // Add before removing — the last remaining measure is held checked, so the
  // selection can never pass through empty.
  await control(page, "Pages read").check();
  await control(page, "Books finished").uncheck();
  await control(page, "Avg book length").uncheck();
  await settled();
  await expect(canvas).toBeVisible();

  // A measure with a bounded coverage window states that in the notes.
  await expect(control(page, "Pages read")).toBeChecked();
  await page
    .getByTestId("chart-notes")
    .getByText("What this chart shows")
    .click();
  await expect(page.getByTestId("chart-caveat").first()).toContainText(
    "progress ledger",
  );
});

test("reads every series at the hovered bucket", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);

  // No hover state in the server-rendered markup — it is client-only, so the
  // first paint has to match it (rule 07).
  await expect(page.getByTestId("chart-tooltip")).toHaveCount(0);

  const bands = page.locator(".cb-hit");
  await expect(bands.first()).toBeAttached();
  await bands.nth(2).hover({ force: true });

  const tip = page.getByTestId("chart-tooltip");
  await expect(tip).toBeVisible();
  // Both series at once, each with its own unit, since they may be on
  // different scales.
  await expect(tip).toContainText("Books finished");
  await expect(tip).toContainText("books");
  await expect(tip).toContainText("Avg book length");
  await expect(tip).toContainText("pages");
});

test("stacks a split count into one column per bucket", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);
  await control(page, "Avg book length").uncheck();
  await control(page, "Split").selectOption("genre");

  // The previous chart stays on screen until its replacement arrives, so a
  // row count is not enough to tell the split has landed — the unsplit chart
  // already has two. Wait for a slice-qualified label, which only a split
  // produces.
  await expect(page.getByTestId("chart-legend")).toContainText(
    "Books finished ·",
  );

  // Stacked bars share one lane, so every bar in a bucket has the same x.
  const xs = await page
    .locator("rect.cb-bar")
    .evaluateAll((nodes) => nodes.map((n) => n.getAttribute("x")));
  const labels = await page.locator("text.cb-xlabel").count();
  expect(xs.length).toBeGreaterThan(labels);
  expect(new Set(xs).size).toBeLessThanOrEqual(labels);
});

test("explains what the chart shows and why the list is narrowed", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  await plotThePair(page);
  const notes = page.getByTestId("chart-notes");
  await expect(notes).toBeVisible();

  // Closed by default, so it never pushes the chart off screen.
  await expect(page.getByTestId("chart-notes-scales")).toBeHidden();
  await notes.getByText("What this chart shows").click();

  // Each measure says what it counts, at which grain, in which unit.
  await expect(notes).toContainText("Books you marked finished");
  await expect(notes).toContainText("Measured per book finished, in books.");

  // Two units on screen, so the notes warn that a crossing means nothing and
  // say which units are holding the scales.
  await expect(page.getByTestId("chart-notes-scales")).toContainText(
    "means nothing",
  );
  await expect(page.getByTestId("chart-notes-availability")).toContainText(
    "books and pages",
  );
});

test("rewrites the notes when the selection changes", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);
  await page
    .getByTestId("chart-notes")
    .getByText("What this chart shows")
    .click();

  // Down to one unit: a free scale, and nothing to misread across axes.
  await control(page, "Avg book length").uncheck();
  await expect(page.getByTestId("chart-notes-availability")).toContainText(
    "still free",
  );
  await expect(page.getByTestId("chart-notes-scales")).not.toContainText(
    "means nothing",
  );

  // A measure with a bounded window states that under its own heading.
  await control(page, "Pages read").check();
  await control(page, "Books finished").uncheck();
  await expect(page.getByTestId("chart-caveat").first()).toContainText(
    "progress ledger",
  );
});

test("zooms to a brushed range and back", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);

  const bands = page.locator(".cb-hit");
  const before = await bands.count();
  expect(before).toBeGreaterThan(3);
  // Nothing to reset before a zoom exists.
  await expect(page.getByTestId("chart-zoom")).toHaveCount(0);

  await brush(page, 1, before - 2);

  const zoomBar = page.getByTestId("chart-zoom");
  await expect(zoomBar).toBeVisible();
  await expect(zoomBar).toContainText(`of ${before} periods`);
  await expect.poll(async () => bands.count()).toBeLessThan(before);

  await page.getByRole("button", { name: "Show all" }).click();
  await expect(zoomBar).toHaveCount(0);
  await expect.poll(async () => bands.count()).toBe(before);
});

test("drops a zoom the regrouped axis can no longer resolve", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  await plotThePair(page);

  const bands = page.locator(".cb-hit");
  const before = await bands.count();
  await brush(page, 1, before - 2);
  await expect(page.getByTestId("chart-zoom")).toBeVisible();

  // Regrouping replaces every bucket key, so the zoom has nothing to resolve
  // against and drops itself rather than framing a different stretch.
  await control(page, "Group by").selectOption("week");
  await expect(page.getByTestId("chart-zoom")).toHaveCount(0);
});

test("says what zooming can and cannot do", async ({ page }) => {
  await gotoReady(page, CHART);
  await plotThePair(page);
  await page
    .getByTestId("chart-notes")
    .getByText("What this chart shows")
    .click();
  const zoomNote = page.getByTestId("chart-notes-zoom");
  await expect(zoomNote).toContainText("Drag across the chart");
  // The limit of a client-side zoom, stated rather than left to be found.
  await expect(zoomNote).toContainText("Group by");
});

test("surfaces a rejected spec instead of failing silently", async ({
  page,
}) => {
  // The server re-validates every spec, so force its rejection path and assert
  // the page reports it rather than sitting on a stale chart.
  await page.route("**/api/rpc/chart-series", (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "something went wrong" }),
    }),
  );

  await gotoReady(page, CHART);
  // The page opens empty and asks for nothing, so the failure only has a
  // request to attach itself to once a measure is picked.
  await control(page, "Books finished").check();
  await expect(page.getByRole("alert")).toBeVisible();
});
