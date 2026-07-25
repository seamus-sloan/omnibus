import type { Page, Route } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The admin log viewer — now the Logs section of Settings (#1324); the legacy
// `/logs` route redirects here. The layout test runs against the real on-disk
// logs the server is already writing (proving AC1 end-to-end); the filter /
// error / pagination tests mock `/api/rpc/logs` after the initial load so the
// rows under assertion are deterministic.
const LOGS = "/settings?section=logs";

const levelFilter = (page: Page) => page.getByLabel("Level");
const moduleFilter = (page: Page) => page.getByLabel("Module");
const applyButton = (page: Page) => page.getByTestId("logs-apply");
const rows = (page: Page) => page.getByTestId("logs-row");

// Build a `LogPage` JSON body for a mocked response.
function logPage(
  records: Array<{
    level: string;
    target: string;
    message: string;
    fields?: string;
  }>,
  opts: { page?: number; per_page?: number; has_more?: boolean } = {},
) {
  return JSON.stringify({
    records: records.map((r, i) => ({
      timestamp: `2026-07-19T08:00:0${i}.000000Z`,
      level: r.level,
      target: r.target,
      message: r.message,
      ...(r.fields ? { fields: r.fields } : {}),
    })),
    page: opts.page ?? 0,
    per_page: opts.per_page ?? 50,
    has_more: opts.has_more ?? false,
  });
}

// Fulfill a POST to the logs RPC with `body`; pass through everything else.
function mockLogs(page: Page, body: string | ((route: Route) => string)) {
  return page.route("**/api/rpc/logs", (route) => {
    if (route.request().method() !== "POST") return route.continue();
    const payload = typeof body === "function" ? body(route) : body;
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: payload,
    });
  });
}

test("renders the log viewer layout from the on-disk logs", async ({
  page,
}) => {
  // The page fires a POST /api/rpc/logs on mount (the framework routes the
  // arg-bearing read as POST); await it explicitly per rule 04 rather than
  // leaning on networkidle.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 200 },
    async () => gotoReady(page, LOGS),
  );

  await expect(
    page.getByRole("heading", { level: 2, name: "Server logs" }),
  ).toBeVisible();
  await expect(page.getByTestId("settings-nav-logs")).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(levelFilter(page)).toBeVisible();
  await expect(moduleFilter(page)).toBeVisible();
  await expect(page.getByLabel("From")).toBeVisible();
  await expect(page.getByLabel("To")).toBeVisible();
  await expect(applyButton(page)).toBeVisible();
  // The server is continuously logging its own request handling, so the
  // real on-disk logs are non-empty — the table renders actual records.
  await expect(page.getByTestId("logs-table")).toBeVisible();
  await expect(rows(page).first()).toBeVisible();
  await expectNavVisible(page);
});

test("filtering by level narrows the results and sends the level in the query", async ({
  page,
}) => {
  await gotoReady(page, LOGS);

  // Mock only the post-Apply request so the rendered rows are deterministic.
  await mockLogs(
    page,
    logPage([
      {
        level: "ERROR",
        target: "omnibus_db::worker::queue",
        message: "worker: task failed",
      },
    ]),
  );

  await levelFilter(page).selectOption("ERROR");
  const { request } = await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 200 },
    async () => applyButton(page).click(),
  );
  // Lock the meaningful part of the query contract without asserting the whole
  // serialized shape (page/per_page defaults ride along).
  expect(request.postDataJSON()).toMatchObject({ query: { level: "ERROR" } });

  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText("worker: task failed");
  await expect(page.getByText("ERROR", { exact: true })).toBeVisible();
});

test("filtering by module sends the module substring in the query", async ({
  page,
}) => {
  await gotoReady(page, LOGS);

  await mockLogs(
    page,
    logPage([
      {
        level: "INFO",
        target: "omnibus::backend::covers",
        message: "served cover",
      },
    ]),
  );

  await moduleFilter(page).fill("omnibus::backend");
  const { request } = await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 200 },
    async () => applyButton(page).click(),
  );
  expect(request.postDataJSON()).toMatchObject({
    query: { module: "omnibus::backend" },
  });

  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText("omnibus::backend::covers");
});

test("paginates to older records and back", async ({ page }) => {
  await gotoReady(page, LOGS);

  // Page 0 has a further page; page 1 is the last. Branch on the requested
  // page so Older/Newer render distinct, deterministic rows.
  await mockLogs(page, (route) => {
    const requestedPage = route.request().postDataJSON()?.query?.page ?? 0;
    return requestedPage >= 1
      ? logPage([{ level: "INFO", target: "a", message: "older-record" }], {
          page: 1,
          has_more: false,
        })
      : logPage([{ level: "INFO", target: "a", message: "newer-record" }], {
          page: 0,
          has_more: true,
        });
  });

  // Apply with no filters to land on the mocked page 0.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 200 },
    async () => applyButton(page).click(),
  );
  await expect(page.getByTestId("logs-page-indicator")).toHaveText("Page 1");
  await expect(page.getByTestId("logs-prev")).toBeDisabled();
  await expect(rows(page).first()).toContainText("newer-record");

  // Older → page 1.
  const { request: olderReq } = await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 200 },
    async () => page.getByTestId("logs-next").click(),
  );
  expect(olderReq.postDataJSON()).toMatchObject({ query: { page: 1 } });
  await expect(page.getByTestId("logs-page-indicator")).toHaveText("Page 2");
  await expect(page.getByTestId("logs-prev")).toBeEnabled();
  await expect(page.getByTestId("logs-next")).toBeDisabled();
  await expect(rows(page).first()).toContainText("older-record");

  // Newer → back to page 0.
  const { request: newerReq } = await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 200 },
    async () => page.getByTestId("logs-prev").click(),
  );
  expect(newerReq.postDataJSON()).toMatchObject({ query: { page: 0 } });
  await expect(page.getByTestId("logs-page-indicator")).toHaveText("Page 1");
  await expect(page.getByTestId("logs-prev")).toBeDisabled();
  await expect(rows(page).first()).toContainText("newer-record");
});

test("rejects an unauthenticated request to the log endpoint (AC4)", async ({
  playwright,
  baseURL,
}) => {
  // A fresh request context carries neither the admin bearer (the suite's
  // default `request` fixture) nor the browser session cookie, so the
  // `require_auth` boundary must turn it away before any log is read.
  const anon = await playwright.request.newContext({ baseURL });
  const response = await anon.post("/api/rpc/logs", {
    data: { query: { page: 0, per_page: 5 } },
  });
  expect(response.status()).toBe(401);
  await anon.dispose();
});

test("shows an error status when the log read fails", async ({ page }) => {
  await gotoReady(page, LOGS);

  await page.route("**/api/rpc/logs", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/logs", expectedStatus: 500 },
    async () => applyButton(page).click(),
  );

  await expect(page.getByTestId("logs-error")).toBeVisible();
  await expect(page.getByTestId("logs-error")).toContainText(
    "Failed to load logs",
  );
});

test("reaches the log viewer from the sidebar Logs item (AC3)", async ({
  page,
}) => {
  await gotoReady(page, "/settings");

  await page.getByTestId("settings-nav-logs").click();

  await expect(page).toHaveURL(/\/settings\?section=logs$/);
  await expect(
    page.getByRole("heading", { level: 2, name: "Server logs" }),
  ).toBeVisible();
  await expect(page.getByTestId("logs-page")).toBeVisible();
});

test("the legacy /logs route redirects into the Logs section", async ({
  page,
}) => {
  await gotoReady(page, "/logs");

  await expect(page).toHaveURL(/\/settings\?section=logs$/);
  await expect(
    page.getByRole("heading", { level: 2, name: "Server logs" }),
  ).toBeVisible();
});
