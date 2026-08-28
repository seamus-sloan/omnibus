// Ownership guard installed into an agent's browser by `driver.sh guard`.
//
// start.md says an agent may only destroy what it added. Until now that was a
// convention in a prompt: every exploration account is an admin, so the server
// will happily let agent-2 delete agent-5's book, and nothing but the agent's
// own compliance stood in the way. This makes it enforced — the request is
// aborted in the browser before it reaches the server.
//
// __OWNED__ is replaced with a JSON array of uuids the actor has a book.add
// entry for, in any run. __ACTOR__ with the actor id.
(() => {
  const owned = new Set(__OWNED__);
  const actor = "__ACTOR__";

  // Endpoints that destroy or restructure a book. Anything book-scoped is
  // gated on the owned set; author and series deletion is refused outright,
  // because start.md forbids it for every agent regardless of ownership.
  const BOOK_SCOPED = /\/api\/(rpc\/(books\/delete-files|merge-books)$|physical\/)/;
  const ALWAYS_REFUSED = /\/api\/rpc\/(author\/delete|cleanup\/delete-entity)/;
  // Undo is destructive and owner-only, but its payload carries a merge_log_id
  // and no uuid, so ownership cannot be read from the request. Refusing it
  // outright would block the very step that found the undo data-loss bug
  // (#2234), so instead it is allowed only to reverse a merge this guard
  // already approved — which was itself ownership-checked.
  const UNDO = /\/api\/rpc\/merge-books\/undo$/;
  // `merge-books/candidates` is a search, not a mutation. It must keep working
  // or the merge dialog cannot be used at all.
  const MERGE_READ = /\/api\/rpc\/merge-books\/candidates$/;

  const uuidsIn = (text) =>
    (text || "").match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g) || [];

  globalThis.__omnibusGuardRefusals = [];
  globalThis.__omnibusGuardApprovedMerges = 0;

  return page.route("**/api/**", (route) => {
    const req = route.request();
    const url = req.url();
    const method = req.method();

    if (method === "GET" || method === "HEAD") return route.continue();

    const refuse = (why, targets) => {
      globalThis.__omnibusGuardRefusals.push({ actor, url, method, why, targets });
      // 403 rather than a transport error: the app renders a permission
      // failure, which is what the agent should observe and journal, instead
      // of a network stack error it would mistake for a bug.
      return route.fulfill({
        status: 403,
        contentType: "application/json",
        body: JSON.stringify({ error: "ownership_guard", why, targets }),
      });
    };

    if (ALWAYS_REFUSED.test(url)) return refuse("author and series deletion are forbidden by the rails", []);

    if (MERGE_READ.test(url)) return route.continue();

    if (UNDO.test(url)) {
      if (globalThis.__omnibusGuardApprovedMerges > 0) {
        globalThis.__omnibusGuardApprovedMerges -= 1;
        return route.continue();
      }
      return refuse("undo has no uuid to check and follows no merge this guard approved", []);
    }

    if (BOOK_SCOPED.test(url)) {
      const targets = [...uuidsIn(req.postData()), ...uuidsIn(url)];
      const unowned = targets.filter((u) => !owned.has(u));
      // No uuid at all in a destructive call means the guard cannot prove
      // ownership — refuse rather than wave it through.
      if (targets.length === 0) return refuse("destructive call carried no book uuid to check", []);
      if (unowned.length > 0) return refuse("actor does not own these books", unowned);
      // Remember an approved merge so its undo can be allowed through.
      if (/merge-books$/.test(url)) globalThis.__omnibusGuardApprovedMerges += 1;
    }
    return route.continue();
  });
})()
