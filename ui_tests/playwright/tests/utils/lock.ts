import { mkdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Cross-worker mutex for E2E sections that mutate shared server state.
//
// Playwright runs a shard's specs across several worker *processes* that all
// hit the same dev server, so two specs merging the same audiobook record in
// parallel corrupt each other's assertions (e.g. merge.spec ghosts "The
// Analytical Audiobook" into an ebook while book_detail's file-picker test is
// merging a second file into it). `mkdir` is atomic on every platform, so a
// lock *directory* under the OS temp dir gives a machine-wide mutex shared by
// every worker in the shard. (Across shards each runner has its own server, so
// there's nothing to serialize and the uncontended lock is a no-op.)

const STALE_MS = 60_000;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Run `fn` while holding a machine-wide lock named `name`. Acquisition spins
 * on an atomic `mkdir`; a lock older than {@link STALE_MS} is treated as
 * abandoned (a crashed worker) and reclaimed. The lock is always released,
 * even if `fn` throws — callers must leave shared state restored before
 * returning so the next holder sees a clean fixture.
 */
export async function withLock<T>(name: string, fn: () => Promise<T>): Promise<T> {
  const dir = join(tmpdir(), `omnibus-e2e-lock-${name}`);
  for (;;) {
    try {
      mkdirSync(dir);
      break;
    } catch {
      try {
        if (Date.now() - statSync(dir).mtimeMs > STALE_MS) {
          rmSync(dir, { recursive: true, force: true });
          continue; // reclaim immediately, then retry the mkdir
        }
      } catch {
        // Released between the failed mkdir and the stat — just retry.
      }
      // Jitter so waiters don't stampede the mkdir in lockstep.
      await sleep(50 + Math.floor(Math.random() * 100));
    }
  }
  try {
    return await fn();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
