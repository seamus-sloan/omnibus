// Extended `test` / `expect` for the omnibus Playwright suite.
//
// Today this just re-exports Playwright's base test runner. It exists so every
// spec imports from a single place — when we need shared state (seeded DB,
// mocked library responses, pre-filled settings), we add a `test.extend(...)`
// here and every spec picks it up automatically.
import { test as base, expect } from "@playwright/test";

export const test = base;
export { expect };
