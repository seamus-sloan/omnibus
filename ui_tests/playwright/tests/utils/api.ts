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
  const matchesUrl = (candidate: string) =>
    typeof opts.url === "string" ? candidate.includes(opts.url) : opts.url.test(candidate);

  const requestPromise = page.waitForRequest(
    (r) => r.method() === opts.method && matchesUrl(r.url()),
  );
  const responsePromise = page.waitForResponse(
    (r) => r.request().method() === opts.method && matchesUrl(r.url()),
  );

  const result = await action();
  const request = await requestPromise;
  const response = await responsePromise;

  if (opts.expectedBody !== undefined) {
    expect(request.postDataJSON()).toEqual(opts.expectedBody);
  }
  if (opts.expectedStatus !== undefined) {
    expect(response.status()).toBe(opts.expectedStatus);
  }

  return { result, request, response };
}
