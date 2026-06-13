## UI / UX Design Review

### Overall Technical Score: 58/100

> Information architecture 14/25 — taxonomy discovery is broken end-to-end (orphaned tag cloud, dead detail chips) and the two immersive surfaces disagree on the nav model. Interaction clarity 12/25 — multiple prominent controls (Add books, listen Bookmark/Sleep) are silent no-ops or open dead-end panels. Visual consistency 11/20 — three parallel token systems and an unmigrated Settings page mean the theme switch doesn't reach several surfaces. Intuitiveness 13/20 — fabricated personal metrics, duplicated detail-page actions, and four different "coming soon" conventions make the app hard to predict. Future-fit 8/10 — the Atrium token set and existing `tag:` search backend give a clear consolidation path, and most debt is additive rather than structural.

---

### High Priority — Tag cloud orphaned, links don't filter

The `/tags` discovery page fails on two axes. First it is unreachable: `Route::TagCloud` appears only in routes.rs:37, and a repo-wide grep finds zero inbound `to: Route::TagCloud` links. Neither top_nav.rs (Library/Authors/Series + disabled Stats span) nor bottom_nav.rs exposes it, and no search result points there, so the only way in is typing the URL. Second, once there, every tag links to the wrong place: tag_cloud.rs:113 renders each tag as `Link { to: Route::Landing {} }`, so clicking a tag under a subtitle that literally says "click to filter" (tag_cloud.rs:59) dumps the user on the unfiltered library home.

This violates F1.8's acceptance criterion ("clicking a tag deep-links to /?tags=fantasy", docs/roadmap/1-8-discovery-pages.md:37). The painful part is that a working tag filter already exists elsewhere: the search palette (search_palette/results.rs:106-108) and full-page search (search.rs:156) both route tag clicks to `Route::Search { query: "tag:{name}" }`, and the `tag:` facet is genuinely parsed in the backend (db/src/books/search.rs:26). So the app ships a working tag filter from the palette and a broken one from the dedicated tag page.

Fix: surface `/tags` from the primary nav (a Browse cluster alongside Authors/Series) and change each tag `Link` to the working `Route::Search { query: format!("tag:{}", name) }`. Note `/?tags=` query-param parsing does not yet exist in landing, so the `tag:` search route is the grounded target today. Also fill or drop the empty `aside { class: "card", aria_hidden: "true" }` placeholder at tag_cloud.rs:72, which renders a visible empty card next to the cloud.

---

### High Priority — Book-detail tag chips are dead ends

On the book-detail page, subject/tag chips render as plain non-interactive list items — `li { key: "{tag}", class: "chip", "{tag}" }` with no `Link` and no handler (book_detail.rs:500-504). Tags are the most natural "more like this" pivot on a detail page: a reader who likes a fantasy title wants to tap "fantasy" and see the rest of the shelf. Here they are inert decoration.

This compounds the orphaned tag-cloud problem. Between the two findings there is no working tag-driven discovery path anywhere a user naturally looks — not the detail page, not the dedicated tag page — even though the `tag:` search filter that powers it already exists (search.rs:156, db/src/books/search.rs:26). F1.8 explicitly frames taxonomy as "a real place users can visit from a book-detail breadcrumb" (docs/roadmap/1-8-discovery-pages.md:13), and this is exactly the breadcrumb that is missing.

Fix: make each detail-page chip a `Link` to `Route::Search { query: format!("tag:{}", tag) }`, reusing the existing working route. This is a cheap change with a high discoverability payoff, and it should land together with the tag-cloud fix so the two surfaces converge on one tag-filter behavior.

---

### High Priority — "Add books" CTA is silent no-op

The top nav renders an "Add books" button styled as the primary CTA (`class: "btn primary sm"`) — visually the loudest control in the chrome, beside the user avatar on every web page — but it has no `onclick`, is not `disabled`, and carries no title/tooltip (top_nav.rs:57-62). Clicking it does nothing: no navigation, modal, toast, or not-allowed cursor.

This is the worst class of dead affordance. The styling actively signals "this is the main thing to do here," then fails silently. On a fresh empty library — where adding books is the obvious next step — new users will click it first and conclude the app is broken. Worse, it violates the convention the rest of the codebase already follows for unbuilt actions: the reader bookmark is `disabled: true` with `title: "Bookmark — coming soon"` (reader/mod.rs:565-571), the format actions are `disabled: true, title: "Send-to-Kindle coming soon"` (format_switcher.rs:98-104), and the Stats nav item is an inert `span { class: "disabled", aria_disabled: "true" }` (top_nav.rs:51).

Uploads are a real roadmap item (F5.3), but exposing an active primary button before it works is premature. Fix: until F5.3, mark it `disabled` with `title="Uploads — coming soon"` and demote from `primary` to `ghost`, or wire it to Settings (the only path by which books enter today) so the button teaches rather than misleads.

---

### High Priority — Reader and listen disagree on chrome

routes.rs documents both `/read/:uuid` (83-92) and `/listen/:uuid` (94-102) as immersive surfaces rendered without ScreenLayout, each owning "its own slim top bar" so the app nav is suppressed. The reader honors this: it renders a purpose-built slim bar (`ReaderTopChrome`, reader/mod.rs:522-582) with a back chevron, centered title, Aa, and a disabled bookmark, suppressing app nav entirely. The listen player does the opposite — it re-mounts the full app `Nav {}` (the web TopNav) inside its own 100vh `lp-root` surface (ready_player.rs:245).

So a user listening on web sees the entire Atrium topbar (brand, Library/Authors/Series, disabled Stats, search trigger, primary Add-books button, user menu) layered over the cinematic player, while the reader gets a clean dedicated bar. Two opposite navigation models for the same "immersive" concept. Worse, the listen page has no dedicated back/close/ESC at all (the listen chrome has only inner panel `on_close` handlers), so the only way out is the app nav links it mounts. Separately, the reader's only escape is `nav.go_back()` (reader/mod.rs:294-297, also bound to ESC at 388-389); opened from a deep link or fresh tab with no history, that strands the user on a bare surface with no app nav.

Fix: pick one model. Give the listen player its own slim bar matching `rd-top` (back + title) and drop the full TopNav from the listen route; and make the reader's back control navigate to an explicit "Back to book"/"Back to library" target rather than history-dependent `go_back`.

---

### High Priority — Listen Bookmark/Sleep open unusable panels

Two of the three listen toolbar buttons (controls.rs:272-289) are always enabled and toggle panels that cannot do anything. The Bookmark button (controls.rs:278-283, no `disabled`) opens a drawer whose entire body is an empty state: "No bookmarks yet — Tap the Bookmark button while listening to save your place" (bookmarks_drawer.rs:38-44). The instruction is circular — the only button it points the user at is the very button that opened this empty drawer, and there is no save path (CRUD ships in F2.3b PR 4, unstarted). The Sleep button is similarly inert: the panel renders a full preset grid but `disabled: !is_off` makes every duration unclickable except "Off," and the End-of-chapter row is `disabled: true` (sleep_panel.rs:42-66). The panel opens, shows durations, and rejects every one.

The Chapters button (the third) does work, which makes the dead two harder to predict — there is no learnable rule for which toolbar buttons act. Presenting fully-rendered, clickable-looking controls that silently reject interaction is more confusing than hiding them.

Fix: until the backing PRs land, render the Bookmark and Sleep toolbar buttons themselves `disabled` with a coming-soon title (matching the reader's disabled bookmark and the book-detail disabled actions) rather than letting them open panels that can't act. If the shells are kept for visual preview, the drawer copy must not instruct an action that doesn't exist. The roadmap (docs/roadmap/2-3b-listen-redesign.md) confirms PR4/PR5 are "inert visual shells."

---

### High Priority — Settings unguarded for non-admins, unthemed

settings.rs is documented "Admin-only" (line 3) but the component renders the full library-path form, Save, Refetch-author-photos, and Extract-chapters buttons to any authenticated user — there is no `is_admin` check anywhere in the component body. It is reachable by everyone: the mobile bottom nav links Settings for all (bottom_nav.rs:19) and the web user-menu Settings row is enabled for all (user_menu.rs:223-229). A non-admin loads a fully interactive admin form and only discovers it is forbidden when an action 403s server-side — a confusing dead end with no up-front "you don't have access" messaging. The book-detail page already demonstrates the right pattern: it resolves `current_user()` into an `is_admin` signal to gate the Merge affordance (book_detail.rs:58-65).

Separately, Settings is the one major surface that did not migrate to the Atrium design system. It uses plain `card`/`settings-form`/`settings-field` markup (settings.rs:72-131) whose CSS hardcodes slate hex — input bg `rgba(30,41,59,.8)`, focus `#3b82f6`, labels `#cbd5e1`, success `#34d399`, error `#f87171` (atrium.css:3105-3170) — none keyed off `data-theme`. The form does not respond to the Dark/Light/Sepia switch at all: on Light or Sepia the user still sees a dark slate form. The page heading also mixes one Atrium primitive (`.card`) with a legacy `h1` + `.subtitle`.

Fix: gate the body on `is_admin` with a friendly non-admin state, and rebuild the form with Atrium `.me-input`/`.me-label`/`.btn` + token-driven status so theme cascades.

---

### High Priority — Three token systems fragment visual language

The design system is actually three coexisting ones, and which you see depends on the page. (1) Atrium: oklch warm-neutral tokens `--bg-0..3`, `--ink-0..3`, `--accent`, Instrument Serif / Geist, with dark/light/sepia variants (atrium.css:21-104) — drives landing, book_detail, discovery, reader, listen, metadata_edit. (2) Auth: a wholly separate `--auth-*` block — slate `rgba(15,23,42,...)` grounds, cyan `#22d3ee` accent, Iowan Old Style serif, Inter sans (atrium.css:2684-2703) — drives login/register, entirely outside the theme system. (3) Legacy globals: a `body` radial slate gradient + Inter + hardcoded `#e5e7eb`/`#94a3b8` (atrium.css:2644-2677), plus the mobile bottom-nav and Settings blocks in raw hex (atrium.css:3086-3170).

The same semantic role (primary accent, error ink, body text) has three different literal values across these worlds, so the product reads as three apps stitched together — and the auth/settings/bottom-nav surfaces ignore the Dark/Light/Sepia switch entirely. This blocks coherent theming for every future screen (stats, journal, shelves, OPDS, uploads) since there is no single source of truth.

Fix: collapse onto the Atrium token set — re-express `--auth-*` and the legacy hex as references to `--bg-*`/`--ink-*`/`--accent`/`--ok`/`--bad`, then migrate Settings, auth, and bottom-nav markup to Atrium primitives so a token change propagates everywhere. Auth pages are also the first/last screen a user sees and ship their own gradient brand mark (atrium.css:2732) versus the Atrium book-spine brand glyph, so unifying the brand mark belongs in the same effort.

---

### Medium Priority — Mobile bottom nav off-palette, no active state

The mobile bottom tab bar diverges from the web top nav in both look and behavior. Behaviorally it is worse than web: `BottomNav` renders four bare `Link`s (Home/Authors/Series/Settings) with no `class` and nothing computes the current route (bottom_nav.rs:14-21), so the `.bottom-nav a.active { color: #22d3ee }` CSS rule (atrium.css:3103) is dead code and a mobile user gets zero "which tab am I on" feedback. The web TopNav does this carefully — it computes `is_library`/`is_authors`/`is_series` from `use_route` and applies an `on` class (top_nav.rs:21-48).

Visually the bar uses the legacy slate/cyan palette (`background: rgba(15,23,42,.95)`, inactive `#94a3b8`, active `#22d3ee`, atrium.css:3087-3103) instead of Atrium tokens, so it never responds to the theme switch. The tab sets also diverge: bottom nav exposes Settings as a primary tab and labels the home route "Home"; the top nav buries Settings in a dropdown, labels the same route "Library," teases a disabled Stats, and adds search + user menu. Mobile also has no search (SearchPaletteHost is web-gated in top_nav.rs:53-54) and no account actions.

Fix: drive an active class from `use_route` mirroring TopNav, replace the hardcoded colors with Atrium tokens, and reconcile the tab set + labels so the two platforms surface the same primary destinations.

---

### Medium Priority — Listen player has no back/close/ESC

The audiobook player is a full 100vh surface (`.lp-root { height:100vh; overflow:hidden }`, atrium.css:2115-2120) with no exit affordance of its own. `ReadyPlayer` (ready_player.rs:241-344) renders cover, transport, toolbar, and overlays but no back button, no close button, and no ESC handler — the only inner close handlers are for the speed/sleep/bookmark/chapters drawers. The only way off the player is the app `Nav {}` it mounts at ready_player.rs:245 (the brand/Library link). A user who opened the player from a book-detail "Listen" CTA has no obvious path back to that book.

This is the inverse of the reader, which has a dedicated `rd-tool` back button (reader/mod.rs:534-546) plus ESC-to-go-back (reader/mod.rs:388-389). Fix: add a dedicated back/close control to the player stage, reusing the reader's `rd-tool` back pattern, plus an ESC handler. This is the navigation-affordance half of the broader reader/listen chrome inconsistency, so fixing them together is cleanest.

---

### Medium Priority — User menu shows fabricated personal data

The avatar dropdown presents a rich account hub, but most of it is hardcoded fake content shown as real. The "Now reading" block always shows "Piranesi / Susanna Clarke / 68% · ch. 22 · 4h 12m left" (user_menu.rs:199-206) regardless of what the user has open. The stat grid hardcodes "Journal 24 entries," "Highlights 412 quotes," "Shelves 3 shared," "Goals 12 / 24 books" (user_menu.rs:215-218). The Admin row shows "all ok" (user_menu.rs:237-238) and Notifications carries a "2" badge (user_menu.rs:247-248).

Of the whole panel only Settings (223-229), Sign out (263-270), and the theme segment (297-336) actually work; Edit-profile, Now-reading, all four stats, Admin, Notifications, and Switch-user are inert `aria-disabled` `<a>`s. For a self-hosted app where the owner knows they have not written 412 highlights, fabricated personal metrics read as broken/fake and erode trust in every other number the app shows. The disabled rows are at least correctly inert (`aria-disabled`, `tabindex=-1`), but inventing concrete values is the wrong empty-state pattern.

Fix: until the backing features (Journal F3.2, Highlights, Shelves F3.5, Goals, Notifications, Stats F3.4) ship, collapse the menu to the three working rows, or render neutral empty/coming-soon states (the `.sp-coming-soon` affix pattern already exists) with zero fabricated values — never invented user-specific data.

---

### Medium Priority — Inconsistent presentation of unbuilt controls

The app surfaces "coming soon" features through at least four different visual conventions, so users can't learn one rule for what's clickable. (a) Honest: the reader bookmark is `disabled: true` with `title: "Bookmark — coming soon"` (reader/mod.rs:565-571), and format actions are `disabled: true, title: "Send-to-Kindle coming soon"` (format_switcher.rs:98-104) — dimmed, not-allowed cursor. (b) Misleading: the listen Sleep/Bookmark toolbar buttons look fully active and open empty/dead panels (controls.rs:272-289 → bookmarks_drawer.rs / sleep_panel.rs). (c) Disabled buttons with no tooltip: the book-detail action rows "Write a journal entry"/"Add a highlight"/"Mark as finished"/"Share or export…" are `disabled` (book_detail.rs:413-428). (d) Inert links: the user-menu stubs are `aria-disabled` `<a>`s, and the Stats nav item is a `span.disabled` (top_nav.rs:51).

Five surfaces, four shapes, for the same concept. The roadmap acknowledges the listen panels are "inert visual shells" (docs/roadmap/2-3b-listen-redesign.md), so this is known debt — but the UX cost is real: users have no reliable signal for what will respond.

Fix: pick one convention — disabled plus a coming-soon tooltip is the clearest — and apply it everywhere, including disabling the listen Sleep/Bookmark buttons until their PRs land.

---

### Medium Priority — Book detail duplicates and mis-gates actions

The detail page surfaces the same actions multiple times in different shapes, forcing the user to guess which is canonical. Editing is offered twice: a pencil "Edit" in the hero title row (book_detail.rs:456-464, testid edit-metadata-hero) and an "Edit metadata…" link in the rail (book_detail.rs:648-653, testid edit-metadata) — both route to the same `MetadataEdit`. Reading/listening is offered twice: the hero CTA row renders Start reading / Start listening / Listen (BdCtaRow, book_detail.rs:512-552), and the rail FormatSwitcher renders per-format Read/Listen again (format_switcher.rs:73-132). Send-to-Kindle appears as a disabled CTA-row button (book_detail.rs:553) and a disabled per-format action (format_switcher.rs:98-104).

There is also a correctness gap: the hero renders the "Send to Kindle"/"Send to Kobo" buttons unconditionally (book_detail.rs:553-554), outside any `has_ebook` guard, so they dangle even on an audio-only book that can be sent nowhere — the read/listen CTAs at 515-552 are gated, but the send buttons are not.

Fix: pick one home for each action — hero pencil for edit OR rail link, not both; read/listen in the CTA row OR the format switcher, not both — and gate Send-to-Kindle/Kobo on `has_ebook` so they don't appear on audio-only books.

---

### Medium Priority — Reader highlights have no management surface

The reader's selection popover lets the user create highlights in five palette colors (selection.rs:41-71; wired in reader/mod.rs:474-516) and they persist plus re-render on reload. But the reader exposes no surface to view, jump to, recolor, or delete a highlight — the popover offers only create swatches, and there is no list/drawer component under pages/reader/ (only aa_panel, selection, typography). Once a highlight is made, the only removal is an internal optimistic rollback on a failed network write (reader/mod.rs:502-513); there is no user-facing undo.

Because any text drag opens the popover, users will accidentally create highlights and be unable to remove them in-place. F2.4 explicitly scopes "a highlight→quote lifecycle (select → highlight → promote to a quote card)… table-of-contents + notes drawers" (docs/roadmap/2-4-reader-experience.md:9), and the reader's bookmark button is `disabled: true` coming-soon (reader/mod.rs:565-571), so the reader currently offers a one-way create-only flow with no TOC and no notes drawer.

Fix: add a highlights/notes drawer (mirroring the listen ChaptersDrawer pattern) with per-highlight delete, plus a TOC drawer. At minimum, allow tapping an existing highlight to re-open the popover with a Remove action so an accidental highlight is recoverable.

---

### Medium Priority — Listen terminal states skip the design system

The listen page invests in designed terminal states — `FailedOverlay`/`PreparingOverlay` use the `lp-overlay` treatment (serif title + mono detail, overlays.rs:22-43) and the drawers have a designed `lp-drawer-empty` empty state. But the orchestrator's own gating states fall back to bare legacy primitives: the loading state is `p.subtitle "Loading…"`, the not-found state is `p.subtitle "Audiobook not found."` + a generic `.btn`, and the error state is `p[role=alert].subtitle` + `.btn` (listen.rs:108-137).

`.subtitle` is the legacy slate class (`color:#94a3b8`, atrium.css:2677) sitting on the legacy body gradient, not a themed Atrium surface. So depending on which path you hit, you either see a polished cinematic overlay or a bare gray sentence on a mismatched background — and this divergence is most visible exactly when something is slow or broken, which is the worst moment for the UI to look unfinished.

Fix: route the loading/not-found/error gates through the same `lp-overlay` (or an Atrium empty-state) treatment the designed overlays already use, so every state of the listen page reads as one coherent surface.

---

### Medium Priority — Loading-state text and styling inconsistent

Loading and empty states are not visually consistent, so the app reads as assembled from separate mockups. The landing page renders `p { class: "library-empty", "Loading..." }` with three ASCII periods and a different CSS class (landing.rs:239), while book_detail, author, series, tag_cloud, metadata_edit, authors_index, series_index, and listen all render `p { class: "subtitle", "Loading\u{2026}" }` with a real ellipsis (e.g. book_detail.rs:119, tag_cloud.rs:35, listen.rs:137). The search page uses "Searching…" (search.rs:58).

None of these are skeletons — they are bare single-line text — so a library of thousands of covers flashes a tiny "Loading..." then snaps to a full grid, which feels jarring on the heaviest page in the app. Empty states are similarly ad hoc (landing's "No ebooks found." via `library-empty` versus the designed `lp-drawer-empty` blocks elsewhere).

Fix: standardize on one glyph + class and ideally a shared Loading/Empty primitive, and add a cover-grid skeleton given how heavy that page is, so type, spacing, and class match everywhere.

---

### Low Priority — Grid sort controls vanish in table view

The landing page offers a cover grid and a power-user table, but the sort affordance is discoverable in completely different places per view with no parity. In Grid mode the toolbar shows a labeled "Sort by" dropdown + direction toggle (toolbar.rs:94-124). In Table mode that entire block is conditionally omitted (`if view_mode == ViewMode::Grid`, toolbar.rs:94) and sorting is done by clicking column headers (table.rs SortableHeader, whose only active cue is `aria_sort` + an accent color).

A user who learns to sort via the dropdown in Grid loses it on switching to Table with no on-screen hint that headers are clickable; conversely a Table user loses the header model in Grid. The axes also differ: the table makes only Title/Author/Series/LastUpdated/Added sortable (table.rs:60-99) while Publisher (82), Published (83), Formats (84), and Language (99) are plain `th`. Both paths feed the same `ViewPrefs.sort_key`/`sort_dir` (landing.rs:246-258, toolbar.rs:37-51), so the underlying state is unified — only the affordance diverges.

Fix: keep the labeled sort cluster visible in both views (reflecting header clicks into it), or add a clear sortable affordance to the table headers; at minimum align the sortable axes across the two views.

---

### Low Priority — Palette always appends a "Coming soon" row

Every search in the command palette appends a fixed "Inside text" group head + a `.sp-coming-soon` "Coming soon" block, rendered unconditionally inside the `if let Some(ref r)` branch (search_palette/results.rs:117-120), so it fires on every result set. Full-text-inside-book search is a real future capability, but a permanent "Coming soon" row on every query adds noise to the most-used discovery surface and trains users to ignore the bottom of the list.

It also has no parity on the full-page `/search` view (search.rs), which omits it — so the same query shows the teaser in the palette but not on the results page, which is itself a small inconsistency between two views of the same feature.

Fix: drop the always-on placeholder (advertise inside-text search in release notes or a one-time hint), or only show it when there were zero structured results so it reads as "nothing matched — inside-text search is coming" rather than a permanent dead row under every success.

---

### Low Priority — Stale atrium.css header denies shipped Sepia

Sepia is fully implemented and exposed: a complete `[data-theme="sepia"]` token block exists (atrium.css:87-104), `Theme::Sepia` round-trips through `as_attr`/`from_attr` (atrium.rs:40-55), the user-menu theme segment exposes a working Sepia button that persists the choice (user_menu.rs:327-336), the reader Aa panel offers it, and the epub.js glue registers a `sepia` rendition theme and applies it via `setTheme` (epub-reader-glue.js:151,259) — so picking Sepia in the reader genuinely restyles the page.

Yet the stylesheet header still says "Two themes today (dark default, light…)" (atrium.css:5) and "Sepia, density, type pairing toggles deferred to F1.9" (atrium.css:15). Low risk since it is a comment, but it misleads the next contributor into thinking Sepia is unbuilt, risking duplicated or conflicting work — exactly the rot that rule 99 (docs sync) targets.

Fix: update the header to state three themes are live and drop the F1.9-deferred line for Sepia.
