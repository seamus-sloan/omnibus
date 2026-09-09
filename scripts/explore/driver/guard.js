// Ownership guard installed into an agent's browser by `driver.sh guard`.
//
// start.md says an agent may only destroy what it added. Until now that was a
// convention in a prompt: every exploration account is an admin, so the server
// will happily let agent-2 delete agent-5's book, and nothing but the agent's
// own compliance stood in the way. This makes it enforced — the request is
// refused in the browser before it reaches the server.
//
// It lives inside the page, as a wrapper around `window.fetch`, and not in
// DevTools request interception. The first version was a `page.route()`
// handler, and that is what killed run r-20260829-01's uploads (#2361):
// routing turns on CDP `Fetch` interception for every request, and Chromium
// then has to copy each request body into one DevTools event before the
// handler can see it — a multi-hundred-MB audiobook took the whole browser
// down with it, before the upload ever reached the app. The wrapper sees the
// same requests (the WASM client's HTTP all goes through `fetch`; nothing in
// the app uses XMLHttpRequest) and never touches a body it is not about to
// inspect, so an upload of any size passes straight through.
//
// State lives in the driver's Node process, reached over two bindings, so a
// navigation resets nothing: `driver.sh refusals` reads
// `globalThis.__omnibusGuardRefusals` here, and the approved-merge count that
// lets an undo through survives the page it was earned on. Re-running the
// guard replaces the owned set and installs nothing twice.
//
// __OWNED__ is replaced with a JSON array of uuids the actor has a book.add
// entry for, in any run. __ACTOR__ with the actor id.
(async () => {
  const owned = __OWNED__;
  const actor = "__ACTOR__";

  globalThis.__omnibusGuardRefusals ||= [];
  globalThis.__omnibusGuardApprovedMerges ||= 0;
  if (!globalThis.__omnibusGuardBound) {
    await page.exposeBinding("__omnibusGuardRefused", (_source, refusal) => {
      globalThis.__omnibusGuardRefusals.push(refusal);
    });
    // "approve" banks a merge the guard let through; "spend" consumes one for
    // an undo and says whether there was one to spend.
    await page.exposeBinding("__omnibusGuardMerge", (_source, op) => {
      if (op === "approve") {
        globalThis.__omnibusGuardApprovedMerges += 1;
        return true;
      }
      if (globalThis.__omnibusGuardApprovedMerges > 0) {
        globalThis.__omnibusGuardApprovedMerges -= 1;
        return true;
      }
      return false;
    });
    globalThis.__omnibusGuardBound = true;
  }

  // Runs inside the page. Playwright serialises it, so it closes over nothing.
  const install = ({ owned, actor }) => {
    window.__omnibusGuardOwned = new Set(owned);
    window.__omnibusGuardActor = actor;
    // A re-guard must *replace* the wrapper, not return early leaving the old
    // one in place: run r-20260908-02 patched this file mid-run, re-ran
    // `driver.sh guard`, and every agent kept the pre-fix rules because of an
    // early return here. The pristine fetch is parked on `window` so a later
    // install can restore it before wrapping again.
    // A page guarded by a *previous* version of this file parked no pristine
    // fetch, so there is nothing to restore and wrapping again would stack a
    // new wrapper on the old one — the old refusals would still win while the
    // command reported success, which is the false sense of safety this
    // replacement exists to remove. Refuse instead, and say what fixes it.
    if (window.__omnibusGuardInstalled && !window.__omnibusGuardOriginalFetch) {
      window.__omnibusGuardStale = true;
      return { installed: false, reason: "stale-guard-cannot-be-replaced" };
    }
    if (window.__omnibusGuardInstalled) window.fetch = window.__omnibusGuardOriginalFetch;
    window.__omnibusGuardStale = false;
    window.__omnibusGuardInstalled = true;

    // Endpoints that destroy or restructure a book. Anything book-scoped is
    // gated on the owned set; author and series deletion is refused outright,
    // because start.md forbids it for every agent regardless of ownership.
    // Only the calls that name a book uuid and destroy something can be
    // ownership-checked, so only they belong here: file deletion, merge, and
    // the two routes that delete a fileless book. A blanket `physical/` — run
    // r-20260908-01's fix for copy removals slipping past — refused the whole
    // surface instead, reads included, because no copy route carries a uuid.
    const BOOK_SCOPED =
      /\/api\/(rpc\/(books\/delete-files|merge-books|physical\/book\/delete)$|physical\/[0-9a-f-]{36}$)/;
    // Reads wearing POST, plus the wishlist. The wishlist is per-user state on
    // any book in the library, so gating it on the owned set would stop an
    // agent wishlisting a book somebody else uploaded — which every reader may
    // do.
    const PHYSICAL_ALLOWED = /\/api\/rpc\/physical\/(copies|wishlist\/(get|add|remove))$/;
    // A copy note or removal names a copy id and no book uuid, and the check-in
    // response carries no copy id either, so ownership cannot be read from the
    // request or learned from an earlier one. Allowed rather than refused — the
    // alternative loses the flow's last two steps entirely — and recorded as an
    // unverified allowance so the runner can see what went through unchecked.
    const COPY_SCOPED = /\/api\/(rpc\/physical\/copies\/(note|delete)$|physical\/copies\/\d+$)/;
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

    // The body as text, read only for a book-scoped call — never for an upload.
    const bodyText = async (input, init) => {
      const body = init && init.body !== undefined && init.body !== null ? init.body : null;
      if (typeof body === "string") return body;
      if (body instanceof URLSearchParams) return body.toString();
      if (body instanceof FormData) {
        return [...body.entries()].map(([k, v]) => (typeof v === "string" ? `${k}=${v}` : k)).join("&");
      }
      if (body === null && input instanceof Request) return input.clone().text();
      return "";
    };

    const originalFetch = window.fetch;
    window.__omnibusGuardOriginalFetch = originalFetch;
    window.fetch = async function (input, init) {
      const raw = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const url = new URL(raw, location.href).href;
      const method = ((init && init.method) || (input instanceof Request ? input.method : "GET")).toUpperCase();
      if (method === "GET" || method === "HEAD" || !/\/api\//.test(url)) {
        return originalFetch.call(this, input, init);
      }

      const refuse = (why, targets) => {
        const refusal = { actor: window.__omnibusGuardActor, url, method, why, targets };
        Promise.resolve(window.__omnibusGuardRefused(refusal)).catch(() => {});
        // 403 rather than a transport error: the app renders a permission
        // failure, which is what the agent should observe and journal, instead
        // of a network stack error it would mistake for a bug.
        const response = new Response(JSON.stringify({ error: "ownership_guard", why, targets }), {
          status: 403,
          headers: { "content-type": "application/json" },
        });
        // A synthetic Response has an empty `url`, and the Dioxus server-function
        // client parses `response.url` before it reads the status — so the
        // refusal surfaced as "relative URL without a base", a transport error
        // an agent cannot tell from an app bug (r-20260908-01). Give it the URL
        // the request was made to, so the app renders the 403 it was built for.
        Object.defineProperty(response, "url", { value: url });
        return response;
      };

      if (ALWAYS_REFUSED.test(url)) return refuse("author and series deletion are forbidden by the rails", []);
      if (PHYSICAL_ALLOWED.test(url)) return originalFetch.call(this, input, init);
      if (COPY_SCOPED.test(url)) {
        const note = {
          actor: window.__omnibusGuardActor,
          url,
          method,
          allowed: true,
          why: "copy-scoped call carries a copy id and no book uuid — allowed unverified",
          targets: [],
        };
        Promise.resolve(window.__omnibusGuardRefused(note)).catch(() => {});
        return originalFetch.call(this, input, init);
      }
      if (MERGE_READ.test(url)) return originalFetch.call(this, input, init);
      if (UNDO.test(url)) {
        if (await window.__omnibusGuardMerge("spend")) return originalFetch.call(this, input, init);
        return refuse("undo has no uuid to check and follows no merge this guard approved", []);
      }
      if (BOOK_SCOPED.test(url)) {
        const targets = [...uuidsIn(await bodyText(input, init)), ...uuidsIn(url)];
        const unowned = targets.filter((u) => !window.__omnibusGuardOwned.has(u));
        // No uuid at all in a destructive call means the guard cannot prove
        // ownership — refuse rather than wave it through.
        if (targets.length === 0) return refuse("destructive call carried no book uuid to check", []);
        if (unowned.length > 0) return refuse("actor does not own these books", unowned);
        // Remember an approved merge so its undo can be allowed through.
        if (/merge-books$/.test(url)) await window.__omnibusGuardMerge("approve");
      }
      return originalFetch.call(this, input, init);
    };
  };

  await page.addInitScript(install, { owned, actor });
  const result = await page.evaluate(install, { owned, actor });
  return result && result.installed === false ? result.reason : "installed";
})()
