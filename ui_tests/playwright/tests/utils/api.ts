import type { Page, Request, Response } from "@playwright/test";
import { expect } from "../fixtures/test";

export type MutationMethod = "POST" | "PUT" | "PATCH" | "DELETE";

export interface ExpectMutationOptions {
  method: MutationMethod;
  url: string | RegExp;
  expectedBody?: unknown;
  expectedStatus?: number;
}

// Arms waiters for a mutating request + its response, runs the user action
// that triggers the call, then asserts payload and status. Every POST / PUT /
// PATCH / DELETE the UI performs must go through this helper before any UI
// assertion — it guarantees the test observed the network contract and waited
// for the response to land before polling the DOM.
export async function expectMutation<T>(
  page: Page,
  opts: ExpectMutationOptions,
  action: () => Promise<T>,
): Promise<{ result: T; request: Request; response: Response }> {
  // RegExp.test() is stateful when the pattern has the `g` or `y` flag, which
  // would cause intermittent misses across repeated calls. Strip those flags.
  const urlPattern =
    typeof opts.url === "string"
      ? null
      : new RegExp(opts.url.source, opts.url.flags.replace(/[gy]/g, ""));
  const matchesUrl = (candidate: string) =>
    urlPattern === null ? candidate.includes(opts.url as string) : urlPattern.test(candidate);

  const requestPromise = page.waitForRequest(
    (r) => r.method() === opts.method && matchesUrl(r.url()),
  );

  const result = await action();
  const request = await requestPromise;
  // Pair the response to the specific request we captured — using request.response()
  // guarantees the pairing even if concurrent mutations (retries, double-click,
  // background autosave) produce additional matching requests.
  const response = await request.response();
  if (!response) {
    throw new Error(`No response received for ${opts.method} ${request.url()}`);
  }

  if (opts.expectedBody !== undefined) {
    expect(request.postDataJSON()).toEqual(opts.expectedBody);
  }
  if (opts.expectedStatus !== undefined) {
    expect(response.status()).toBe(opts.expectedStatus);
  }

  return { result, request, response };
}
