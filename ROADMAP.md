# Omnibus Roadmap

## Vision

Omnibus is a self-hosted book library for personal use — the ebook and audiobook equivalent of Plex/Jellyfin. It scans a local directory of ebooks (epub) and audiobooks (m4a), organizes them into configurable libraries filtered by metadata, and lets authenticated users browse, read, listen, journal, and rate books. It aims to go beyond what Calibre-Web and AudioBookShelf offer, with first-class device sync, in-browser reading and listening, cross-device progress tracking, and a native mobile app.

---

## Features

### 1. Book Scanning

**Issue Title:** `feat: implement book scanner with epub and m4a metadata extraction`

**Description:**
The foundation of the library. Omnibus scans one or more configured directories for epub and m4a files, extracts metadata from each, and persists it to SQLite. This is a prerequisite for every other feature.

**Implementation Plan:**
- Add `epub` crate for parsing epub files and extracting OPF metadata and cover art
- Add `mp4ameta` crate for reading m4a/AAC tags (title, author, narrator, etc.)
- Create `books` and `scan_paths` tables in SQLite
- Build a `scanner` module that walks directory trees recursively
- Extract per-book: title, author, series, series index, genre, cover art, description, published date, file path, file type
- Store cover art as files in a `covers/` cache directory; store the path in the DB
- Expose `POST /api/admin/scan` to trigger a re-scan on demand
- On re-scan: update existing records by file path, add new books, leave removed books flagged as missing

**Expected Results:**
- Books in the configured folder(s) appear in the database after scanning
- Metadata and cover art are correctly attributed to each book
- Re-scanning is safe to run repeatedly without creating duplicates

**Acceptance Criteria:**
- [ ] Scanning a directory of epub files populates `books` with correct metadata
- [ ] Scanning a directory of m4a files populates `books` with correct metadata
- [ ] Re-scanning updates existing records and adds new ones without duplicates
- [ ] Cover art is extracted and served at a stable URL
- [ ] Books with missing metadata fields store nulls without panicking
- [ ] Books removed from disk are flagged as missing rather than deleted

---

### 2. Libraries

**Issue Title:** `feat: configurable named libraries with metadata filter rules`

**Description:**
Users can create multiple named libraries (e.g. "Sci-Fi", "Fantasy Audiobooks", "Sarah's Books"), each defined by filter rules on book metadata. Libraries are the primary way users navigate their collection.

**Implementation Plan:**
- Add `libraries` and `library_filters` tables to SQLite
- Filter rule schema: `{ field: "genre" | "author" | "series" | "type" | "owner", operator: "eq" | "contains", value: string }`
- Library resolver: query books applying all active filter rules (AND logic within one library)
- UI: library management page — create, rename, and delete libraries
- UI: filter rule builder — add/remove rules with field/operator/value inputs
- Route: `GET /library/:id` returns the filtered book list for that library

**Expected Results:**
- A library called "Fantasy Audiobooks" shows only audiobooks where genre contains "Fantasy"
- Multiple libraries coexist and each returns the correct subset of books

**Acceptance Criteria:**
- [ ] Libraries can be created, renamed, and deleted
- [ ] At least 5 filterable fields: type, genre, series, author, owner
- [ ] Filter rules within a library combine with AND logic
- [ ] A library with no filters shows all books
- [ ] An empty state is shown when no books match the filters

---

### 3. Library Views

**Issue Title:** `feat: sortable table view and cover grid view for library browsing`

**Description:**
Two ways to browse a library: a sortable table for power users and a cover grid for visual browsing. The selected view is remembered per library.

**Implementation Plan:**
- Table view: `<table>` with sortable columns (title, author, series, genre, type, published date)
- Cover grid view: CSS grid of cover `<img>` cards with title and author below each
- View toggle button in the library header; preference stored in `localStorage` keyed by library ID
- Sorting handled server-side via `?sort=title&dir=asc` query params
- Clicking a row or cover card navigates to the book detail page

**Expected Results:**
- Both views display the same books for a given library
- Sorting works correctly in table view
- View preference persists between page loads

**Acceptance Criteria:**
- [ ] Table view and cover grid view show the same books
- [ ] Table view is sortable by: title, author, series, genre, type
- [ ] Cover grid shows cover art with a placeholder fallback if none is available
- [ ] View preference persists per library via `localStorage`
- [ ] Both views navigate to the book detail page on click

---

### 4. Book Detail Page

**Issue Title:** `feat: book detail page with full metadata display and navigation`

**Description:**
Clicking any book in a library view opens a detail page showing all available metadata and cover art, with a breadcrumb back to the originating library. This page also hosts the ratings, journaling, and suggestions sections.

**Implementation Plan:**
- Route: `GET /book/:id`
- Query book record from DB including all metadata fields
- Render: large cover image, title, author, series + position, published date, description, genre, type badge (Ebook / Audiobook)
- "Read" or "Listen" action button depending on type (routes to epub reader or audio player)
- Breadcrumb: `?from=library_id` query param carries the referring library back
- Graceful placeholder for any missing field (cover, description, series, published date)
- Ratings & Journaling section rendered below metadata (see feature 5)
- Book Suggestions section rendered below ratings (see feature 11)

**Expected Results:**
- All available metadata is displayed clearly
- Read/Listen action is prominent and correct for the book type
- Navigating back returns the user to the correct library

**Acceptance Criteria:**
- [ ] Page renders correctly for any book in the database
- [ ] All populated metadata fields are displayed
- [ ] Cover art is shown at an appropriate size with a fallback for missing covers
- [ ] Missing optional fields (series, description, published date) degrade gracefully
- [ ] "Read" button shown for epub, "Listen" button shown for m4a
- [ ] Breadcrumb navigates back to the referring library

---

### 5. Ratings & Journaling

**Issue Title:** `feat: per-user star ratings and markdown journal entries on book detail page`

**Description:**
Authenticated users can rate a book (0–5 stars in 0.5 increments), write a personal markdown journal entry, and attach media. Other users' ratings and entries are visible below.

**Implementation Plan:**
- Schema: `ratings` (`user_id`, `book_id`, `stars` REAL); `journal_entries` (`user_id`, `book_id`, `content` TEXT, `media_urls` JSON array)
- Star rating UI: 10 clickable half-star SVG buttons, saves via `POST /api/books/:id/rating`
- Journal: `<textarea>` with a live markdown preview toggle, saved via `POST /api/books/:id/journal`
- Media upload: `POST /api/books/:id/journal/media` stores files in a `media/` directory; returns URL inserted into markdown
- Aggregate: compute and display average rating + total count above individual entries
- Other users' entries displayed read-only below the current user's section

**Expected Results:**
- User can rate and journal a book; both persist across sessions
- Other users' ratings are visible
- Aggregate rating reflects all user ratings

**Acceptance Criteria:**
- [ ] Star rating supports 0–5 in 0.5 increments (11 distinct states)
- [ ] Rating persists per user per book and is pre-selected on return
- [ ] Aggregate average rating and count are displayed
- [ ] Journal entry supports markdown input with a rendered preview
- [ ] Media files can be attached and appear inline in the markdown preview
- [ ] Other users' ratings and journal entries are shown read-only

---

### 6. Auth & Users

**Issue Title:** `feat: user authentication with account creation and session management`

**Description:**
Secure multi-user access. Users register, log in, and have all personal data (ratings, journals, progress, Kindle address) scoped to their account. The first registered user becomes admin.

**Implementation Plan:**
- Schema: `users` (`id`, `username`, `email`, `password_hash`, `role` [admin/user], `created_at`, `disabled` BOOL)
- Password hashing: `argon2` crate
- Sessions: signed cookie via `tower-sessions` backed by SQLite
- Routes: `GET/POST /login`, `GET/POST /register`, `POST /logout`
- `auth_required` Axum extractor that rejects unauthenticated requests with a redirect to `/login`
- `admin_required` extractor for admin-only routes
- First user to register receives `role = admin`

**Expected Results:**
- Users can register, log in, and log out
- Unauthenticated requests are redirected to `/login`
- All personal data is correctly isolated per user account

**Acceptance Criteria:**
- [ ] Registration with username + email + password creates an account
- [ ] Login with correct credentials establishes a persistent session
- [ ] Invalid credentials return a clear error without revealing which field is wrong
- [ ] Protected routes redirect to `/login` when unauthenticated
- [ ] Logout invalidates the session cookie
- [ ] First registered user receives the admin role automatically

---

### 7. In-Browser Epub Reader

**Issue Title:** `feat: in-browser epub reader with per-user progress tracking`

**Description:**
Users read epub books directly in the browser. Reading position is saved server-side per user and resumes automatically from any browser or device.

**Implementation Plan:**
- Backend: `GET /api/books/:id/epub` serves the epub file (authenticated, streams with appropriate headers)
- Frontend: integrate `epub.js` for in-browser rendering and CFI-based pagination
- Schema: `epub_progress` (`user_id`, `book_id`, `cfi` TEXT — epub Canonical Fragment Identifier)
- Auto-save: debounced `POST /api/books/:id/progress` on each page turn
- Reader UI: full-screen with minimal chrome — prev/next page, chapter list sidebar, font size control, light/dark theme toggle, close button back to detail page
- Route: `GET /read/:id`

**Expected Results:**
- Clicking "Read" opens the epub in a clean reader
- Position is saved automatically and resumed on return

**Acceptance Criteria:**
- [ ] Epub renders correctly in Chrome, Firefox, and Safari
- [ ] Pagination and chapter sidebar navigation both work
- [ ] Font size can be increased and decreased
- [ ] Light and dark reading themes are available
- [ ] Position is saved after each page turn
- [ ] Returning to a book resumes from the exact saved position
- [ ] Progress syncs across browsers for the same user account

---

### 8. In-Browser Audiobook Player

**Issue Title:** `feat: in-browser audiobook player with speed control, sleep timer, and progress sync`

**Description:**
Users stream m4a audiobooks in the browser with full playback controls: variable speed, sleep timer, chapter navigation, and server-synced progress that resumes from any device.

**Implementation Plan:**
- Backend: `GET /api/books/:id/audio` streams m4a with HTTP range request support for seeking
- Frontend: HTML5 `<audio>` element wrapped in a custom player UI component
- Playback speed: set `audio.playbackRate` from a selector (0.5×, 1×, 1.2×, 1.4×, 1.5×, 1.7×, 2×)
- Sleep timer: JS `setTimeout` to pause after 30 min / 1 hr / 2 hr; visible countdown in the player UI
- Chapters: parse chapter markers from m4a during scan, store in `book_chapters` table; render as a clickable list in the player
- Schema: `audio_progress` (`user_id`, `book_id`, `position_seconds` REAL)
- Progress saved every 10 seconds and on pause via `POST /api/books/:id/progress`
- Route: `GET /listen/:id`

**Expected Results:**
- Audio streams and plays with seeking support
- Speed, sleep timer, and chapter navigation all work
- Progress resumes from the last position on any device

**Acceptance Criteria:**
- [ ] m4a streams correctly with HTTP range request support (seeking works)
- [ ] All 7 speed options change playback rate correctly
- [ ] Sleep timer pauses playback at the configured interval with a visible countdown
- [ ] Chapter list is displayed and navigable when chapter markers are present in the file
- [ ] Position is saved every 10 seconds and on pause
- [ ] Returning to an audiobook resumes from the saved position
- [ ] Progress syncs across browsers for the same user account

---

### 9. Kobo Sync (OPDS)

**Issue Title:** `feat: OPDS 1.2 catalog server for native Kobo device sync`

**Description:**
Omnibus serves an OPDS 1.2 Atom feed so Kobo e-readers can browse and download books natively over the local network — no KOReader, no plugins, no USB cable required.

**Implementation Plan:**
- Implement OPDS 1.2 Atom feed at `GET /opds` (root catalog listing all libraries) and `GET /opds/library/:id` (per-library feed)
- Each book entry: title, author, cover image link, epub download link, content type `application/epub+zip`
- Authentication: HTTP Basic Auth on all `/opds/*` routes (Kobo supports this natively)
- Pagination: OPDS `next` link using `?page=N` for libraries with more than 50 entries
- Admin settings UI: display the OPDS URL and Basic Auth setup instructions for Kobo
- Only epub files are served (Kobo does not support m4a)

**Expected Results:**
- A Kobo can add Omnibus as a search catalog, browse libraries, and download books directly
- No third-party tools needed on the Kobo

**Acceptance Criteria:**
- [ ] `GET /opds` returns valid OPDS 1.2 Atom XML
- [ ] Each library is accessible as a sub-catalog feed
- [ ] Each book entry includes a cover thumbnail link, title, author, and epub download URL
- [ ] Epub files download and open correctly on a physical Kobo device
- [ ] HTTP Basic Auth protects all `/opds` routes and is accepted by Kobo
- [ ] Pagination works correctly for libraries with more than 50 books

---

### 10. Kindle Sync

**Issue Title:** `feat: Send to Kindle delivery via SMTP with per-library auto-sync`

**Description:**
Users configure their Send to Kindle email address in account settings. They can deliver any book on demand with a single button, or enable automatic delivery of new books added to a library.

**Implementation Plan:**
- Schema: add `kindle_email` to `users`; add `kindle_auto_sync` table (`user_id`, `library_id`); add `kindle_deliveries` table for delivery audit log
- Admin settings: SMTP configuration (host, port, username, password, from address) stored encrypted in `server_config`
- User settings: input to set/update Send to Kindle email address
- Book detail page: "Send to Kindle" button → `POST /api/books/:id/send-to-kindle`
- Backend: send epub as email attachment via configured SMTP (Amazon accepts epub directly)
- Auto-sync: after each scan, for each newly discovered book, check `kindle_auto_sync` and deliver to matching users
- Delivery success/failure recorded in `kindle_deliveries` and surfaced in user account page

**Expected Results:**
- "Send to Kindle" on any book delivers it to the user's Kindle within minutes
- Auto-sync delivers new matching books without any manual action after a scan

**Acceptance Criteria:**
- [ ] SMTP settings can be saved and tested with a test delivery in admin settings
- [ ] User can configure and update their Send to Kindle email address
- [ ] "Send to Kindle" button on the book detail page delivers the epub
- [ ] Delivery success or failure is shown to the user
- [ ] Auto-sync delivers newly scanned books to users with the library enabled
- [ ] epub format is used directly (no mobi/azw3 conversion required)

---

### 11. Book Suggestions

**Issue Title:** `feat: book suggestions on detail page via local metadata, OpenLibrary, and Hardcover`

**Description:**
The book detail page shows a "You might also like" section. Suggestions draw from local library metadata by default, with optional OpenLibrary integration (no key needed) and Hardcover (user-supplied API key).

**Implementation Plan:**
- Local suggestions: query books sharing author, series, or genre, ranked by overlap count, limit 6
- OpenLibrary: `GET https://openlibrary.org/search.json?author=...&subject=...`; prefer results that exist in the local library, fall back to external links
- Hardcover: query recommendations API using per-user API key stored in `users.hardcover_api_key`; displayed only for the authenticated user
- Priority: Hardcover (if configured) → OpenLibrary → local only
- Cache OpenLibrary and Hardcover results in a `suggestion_cache` table with a 24-hour TTL
- UI: horizontal scroll row of cover cards below the ratings section

**Expected Results:**
- 3–6 relevant suggestions appear on every book detail page
- External integrations degrade gracefully when unavailable or unconfigured

**Acceptance Criteria:**
- [ ] Local suggestions appear with no internet connection
- [ ] OpenLibrary suggestions appear when online and no Hardcover key is set
- [ ] Hardcover suggestions appear when a valid API key is configured in user settings
- [ ] External API results are cached for 24 hours to avoid excessive requests
- [ ] Clicking a suggestion navigates to the book detail page if it exists in the library, or shows an external link otherwise
- [ ] Suggestions section is hidden gracefully when no results are available

---

### 12. Admin Settings

**Issue Title:** `feat: admin settings panel for server configuration and user management`

**Description:**
An admin-only panel for configuring the server: scan folder paths, SMTP for Kindle delivery, manual re-scan trigger, and full user account management.

**Implementation Plan:**
- Route: `GET /admin` — gated to `role = admin` via the `admin_required` extractor
- Scan paths: list configured directories from `scan_paths` table; add/remove via text input + button
- SMTP config: form for host, port, username, password, from address; password stored encrypted in `server_config`; "Send test email" button
- Manual re-scan: "Scan Now" button → `POST /api/admin/scan`; show progress or completion status
- User management: table of all users with actions — disable/enable account, reset password (generates a one-time token), promote/demote admin role
- All changes take effect immediately without a server restart

**Expected Results:**
- Admin can update scan paths and trigger a scan from the UI
- Admin has full control over user accounts from a single page

**Acceptance Criteria:**
- [ ] `/admin` returns 403 for non-admin users
- [ ] Scan paths can be added and removed; changes persist across server restarts
- [ ] SMTP config can be saved and verified with a test delivery
- [ ] Manual re-scan can be triggered from the UI and reports success or failure
- [ ] Admin can disable a user account, preventing login
- [ ] Admin can reset a user's password via a one-time token

---

### 13. Mobile App (Dioxus Native)

**Issue Title:** `feat: native iOS and Android app using Dioxus with offline support and background audio`

**Description:**
A native iOS and Android app built with Dioxus — sharing the same Rust codebase as the web frontend. The app provides full feature parity, plus offline book downloads, background audio playback, and system media controls.

**Implementation Plan:**
- Add `dioxus-mobile` crate target; configure shared component library between web and mobile using Cargo features to gate SSR vs native rendering
- First-launch configuration screen for server URL and login
- Offline storage: on-device SQLite cache of book metadata; file download to local app storage
- Background audio: platform audio APIs via Dioxus native bridge — `AVAudioSession` on iOS, `MediaSession` on Android
- System media controls: lock screen player, AirPlay / Cast support
- Progress sync: push local progress to server when online; optimistic local writes when offline
- Build pipeline: `cargo build --target aarch64-apple-ios` and `aarch64-linux-android`

**Expected Results:**
- App connects to any Omnibus server, authenticates, and provides the full library experience
- Reading and listening work fully offline after books are downloaded
- Audio continues playing in the background with lock screen controls

**Acceptance Criteria:**
- [ ] App connects to a user-configured Omnibus server URL and authenticates
- [ ] Library browsing, book detail page, epub reader, and audiobook player all function in the app
- [ ] Books can be downloaded for offline reading and listening
- [ ] Audiobook playback continues in the background when the app is minimized
- [ ] Lock screen / notification media controls (play, pause, skip chapter) work on both platforms
- [ ] Playback and reading progress syncs to the server when internet is available
- [ ] App builds successfully for both iOS (`aarch64-apple-ios`) and Android (`aarch64-linux-android`) targets

---

## Stack

| Layer        | Technology                                          |
|--------------|-----------------------------------------------------|
| Backend      | Rust / Axum                                         |
| Web frontend | Dioxus SSR + vanilla JS                             |
| Mobile app   | Dioxus Native (iOS / Android, shared Rust codebase) |
| Database     | SQLite (sqlx)                                       |
| Device sync  | OPDS 1.2 (Kobo), Send to Kindle email (Kindle)      |
| Formats      | epub (ebook), m4a (audiobook)                       |
| Book data    | OpenLibrary API, Hardcover API (optional)           |
