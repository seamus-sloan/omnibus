# Feature map — iOS

**Start here to find code; this file answers "where does X live", not "why".**

One row per user-visible feature of the native SwiftUI client, giving its views,
its service, the REST route it calls, how it behaves offline, and its test. For
*why* a module is shaped the way it is, read
[architecture.md](architecture.md#omnibus-ios). For the rules a change must obey,
read [.claude/rules/](../.claude/rules/) — especially
[08-offline-writes.md](../.claude/rules/08-offline-writes.md), which governs the
Offline column below.

`omnibus-ios/` is an **Xcode project, not a Cargo crate** — invisible to
`cargo build` / `just lint` / `just test`. Build and test it with `just ios-build`
/ `just ios-test` / `just ios-test-ui`. The server side of every route below lives
in the Rust workspace — see [feature-map-web.md](feature-map-web.md) to find the
handler.

## Reading a row

Every path is **relative to `omnibus-ios/`** and complete, so it can be opened as
written: `omnibus/Reader/ReaderView.swift`, `omnibusTests/ReaderEchoTests.swift`.
A path ending in `/` means the whole directory is that feature.

The **Offline** column is the rule-08 contract, and it is not a description of
current behaviour — it's a constraint on what you may add:

| Value | Means |
|---|---|
| `queued` | Goes through `SyncEngine`'s outbox; applies optimistically, replays on reconnect |
| `direct` | Calls `APIClient` straight through and **must fail visibly** — never `try?`-swallowed |
| `cached` | Read served from the local replica first, revalidated in the background |
| `local` | Never touches the server |

A write that is `direct` is that way because it failed one of rule 08's four
tests. Do not "improve" it into the outbox without re-running them.

## Shell, connection, account

| Feature | Views | Service / model | Route | Offline | Tests |
|---|---|---|---|---|---|
| App shell, tabs, phase routing | `omnibus/App/`, `omnibus/Design/Components/OmnibusTabBar.swift` | — | — | `local` | `omnibusUITests/omnibusUITests.swift` |
| Server connect | `omnibus/Features/Auth/ServerConnectView.swift` | `omnibus/Services/AuthService.swift` | `/api/_health` | `direct` | — |
| Login / register | `omnibus/Features/Auth/LoginView.swift` | `omnibus/Services/AuthService.swift`, `omnibus/Networking/TokenStore.swift` | `/api/auth/login`, `/register`, `/logout`, `/me` | `direct` | — |
| Account screen | `omnibus/Features/Account/AccountView.swift` | `omnibus/Services/AuthService.swift` | `/api/account/kindle-email`, `/api/account/hidden-formats` | `direct` | `omnibusTests/HiddenFormatsTests.swift` |
| Profile & avatar | `omnibus/Features/Account/ProfileEditSheet.swift` | `omnibus/Services/AuthService.swift` | `/api/account/profile`, `/api/account/avatar` | `direct` | `omnibusTests/ProfileDraftTests.swift`, `omnibusTests/AvatarAttributionTests.swift` |
| HTTP plumbing | — | `omnibus/Networking/` | all | — | `omnibusTests/MultipartBodyTests.swift` |
| Wire types | — | `omnibus/Models/Models.swift`, `omnibus/Models/KnownFormats.swift` | all | — | — |

## Library & browse

| Feature | Views | Service / model | Route | Offline | Tests |
|---|---|---|---|---|---|
| Library grid & categories | `omnibus/Features/Library/LibraryView.swift`, `omnibus/Features/Library/LibraryCategory.swift`, `omnibus/Features/Library/BookContextMenu.swift` | `omnibus/Services/LibraryService.swift` | `/api/ebooks` | `cached` | `omnibusTests/LibraryPollTests.swift` |
| Continue-reading rail | `omnibus/Features/Library/ContinueHero.swift` | `omnibus/Services/UserDataService.swift` | `/api/progress/recent` | `cached` | `omnibusTests/ContinueRailTests.swift` |
| Search | `omnibus/Features/Search/`, `omnibus/Design/Components/SearchField.swift` | `omnibus/Services/LibraryService.swift` | `/api/search`, `/api/search/palette` | `cached` + offline substring | `omnibusTests/SuggestionPoolTests.swift` |
| Authors / series / tags / genres | `omnibus/Features/Discovery/DiscoveryViews.swift` | `omnibus/Services/LibraryService.swift` | `/api/authors*`, `/api/series*`, `/api/tags`, `/api/genres` | `cached` | — |
| Shelves | `omnibus/Features/Shelves/` | `omnibus/Services/UserDataService.swift` | `/api/shelves*` | membership `queued`, create `direct` | `omnibusTests/LibraryShelfRailTests.swift` |
| Book detail | `omnibus/Features/BookDetail/BookDetailView.swift`, `omnibus/Features/BookDetail/BookHero.swift` | `omnibus/Services/LibraryService.swift`, `omnibus/Services/UserDataService.swift` | `/api/ebooks/{uuid}` | `cached` | — |
| Covers & remote images | `omnibus/Design/Components/BookCover.swift`, `omnibus/Design/Components/RemoteImage.swift` | — | `/api/covers/{uuid}`, `/api/thumbs/{uuid}/{size}` | disk-cached | — |

> `omnibus/Features/BookDetail/BookDetailView.swift` is ~1,000 lines and is the
> hub for ratings, read status, genres, journals, wishlist, bookmarks, and
> suggestions. Grep inside it before assuming a book-detail concern has its own
> file.

## Reading & listening

| Feature | Views | Service / model | Route | Offline | Tests |
|---|---|---|---|---|---|
| EPUB reader | `omnibus/Reader/ReaderView.swift`, `omnibus/Reader/ReaderWebView.swift`, `omnibus/Reader/ReaderChrome.swift`, `omnibus/Reader/Web/` | — | `/api/ebooks/{uuid}/file` | download-backed | `omnibusTests/ReaderEchoTests.swift`, `omnibusTests/ReaderRebootTests.swift` |
| Reader settings & typography | `omnibus/Reader/ReaderSettingsSheet.swift`, `omnibus/Reader/ReaderIndicators.swift` | — | — | `local` | `omnibusTests/ReaderSettingsTests.swift`, `omnibusTests/ReaderIndicatorsTests.swift` |
| Table of contents | `omnibus/Reader/ReaderContentsSheet.swift` | — | — | `local` | — |
| Comic (CBZ) reader | `omnibus/Comic/` | — | `/api/ebooks/{uuid}/file` | download-backed | `omnibusTests/ComicArchiveTests.swift`, `omnibusTests/ComicPositionTests.swift` |
| Audiobook player | `omnibus/Features/Player/AudioPlayer.swift`, `omnibus/Features/Player/PlayerView.swift`, `omnibus/Features/Player/PlayerScrubber.swift`, `omnibus/Features/Player/ChapterTimeline.swift` | `omnibus/Services/LibraryService.swift` | `/api/audiobooks/{uuid}/manifest` | download-backed | `omnibusTests/ChapterTimelineTests.swift`, `omnibusTests/RateAdjustedTimeTests.swift` |
| Sleep timer & speed | `omnibus/Features/Player/PlayerSheets.swift` | — | — | rate `queued` | `omnibusTests/PlayerSheetsTests.swift` |
| Car mode | `omnibus/Features/Player/CarModeView.swift` | — | — | `local` | — |
| Progress sync | — | `omnibus/Offline/PositionSync.swift`, `omnibus/Offline/PositionPushThrottle.swift`, `omnibus/Services/UserDataService.swift` | `/api/progress`, `/api/progress/{uuid}` | `queued` (coalesced) | `omnibusTests/PositionPushThrottleTests.swift` |
| Highlights & notes | `omnibus/Reader/AnnotationMenu.swift`, `omnibus/Reader/ReaderSelectionLayer.swift` | `omnibus/Services/UserDataService.swift` | `/api/highlights*` | `queued` | `omnibusTests/HighlightsListTests.swift`, `omnibusTests/ReaderSelectionTests.swift` |
| Quote cards | `omnibus/Reader/QuoteCardView.swift` | — | — | `local` | — |
| Bookmarks | `omnibus/Reader/ReaderContentsSheet.swift` | `omnibus/Services/UserDataService.swift` | `/api/bookmarks*` | `queued` | — |
| Read status (auto) | `omnibus/Reader/ReadStatusAuto.swift` | `omnibus/Services/UserDataService.swift` | `/api/read-status*` | `queued` | `omnibusTests/ReadStatusAutoTests.swift` |
| Cross-format link & resume | `omnibus/Features/BookDetail/AlignmentSheet.swift`, `omnibus/Features/BookDetail/AudioFilePicker.swift` | `omnibus/Services/UserDataService.swift` | `/api/books/{uuid}/cross-format-link`, `/cross-format-resume`, `/alignment`, `/sync-point` | `direct` | `omnibusTests/CrossFormatTests.swift` |

## Personal shelf data

| Feature | Views | Service / model | Route | Offline | Tests |
|---|---|---|---|---|---|
| Star ratings | `omnibus/Features/BookDetail/BookDetailView.swift`, `omnibus/Design/Components/Primitives.swift` | `omnibus/Services/UserDataService.swift` | `/api/ratings`, `/api/ratings/others/{uuid}` | `queued` | — |
| Reading journals | `omnibus/Features/BookDetail/JournalViews.swift` | `omnibus/Services/UserDataService.swift` | `/api/journals*` | `queued` | — |
| Reading stats | `omnibus/Features/Stats/StatsView.swift` | `omnibus/Services/UserDataService.swift` | `/api/stats` | `cached` | — |
| Physical check-in | `omnibus/Features/CheckIn/` | `omnibus/Models/MetadataLookup.swift` | `/api/scan/resolve`, `/search`, `/resolve-meta`, `/check-in` | `direct` (fails rule 08 test 3) | `omnibusTests/CheckInFlowTests.swift`, `omnibusTests/ScanCodecTests.swift` |
| Wishlist | `omnibus/Features/BookDetail/WishlistSection.swift` | `omnibus/Services/UserDataService.swift` | `/api/physical/{uuid}/wishlist` | `direct` | `omnibusTests/WishlistSectionTests.swift` |
| Store links | `omnibus/Features/BookDetail/StoreLink.swift` | — | — | `local` | `omnibusTests/StoreLinkTests.swift` |
| Suggestions | `omnibus/Features/BookDetail/BookDetailView.swift` | `omnibus/Services/LibraryService.swift` | `/api/ebooks/{uuid}/suggestions` | `cached` | — |

## Editing & upload

| Feature | Views | Service / model | Route | Offline | Tests |
|---|---|---|---|---|---|
| Metadata editing | `omnibus/Features/Settings/MetadataEditView.swift`, `omnibus/Features/Settings/MetadataDraft.swift` | `omnibus/Models/Models.swift` | overrides via `/api/ebooks/{uuid}` | `direct` | `omnibusTests/MetadataDraftTests.swift` |
| Provider metadata fetch | `omnibus/Features/Settings/MetadataFetchSheet.swift`, `omnibus/Features/Settings/MetadataFetch.swift`, `omnibus/Features/Settings/MetadataFetchCards.swift` | `omnibus/Models/MetadataLookup.swift` | `/api/metadata/providers` | `direct` | `omnibusTests/MetadataFetchTests.swift` |
| Add books (upload) | `omnibus/Features/AddBooks/` | `omnibus/Services/UploadService.swift`, `omnibus/Services/UploadManager.swift` | `/api/uploads/ebooks` | `direct` | `omnibusTests/UploadFlowTests.swift`, `omnibusTests/UploadStagingTests.swift` |
| Settings screen | `omnibus/Features/Settings/SettingsView.swift` | `omnibus/Services/AuthService.swift` | `/api/settings` | `direct` | — |

## Offline infrastructure

Rule [08](../.claude/rules/08-offline-writes.md) governs the outbox; rule
[09](../.claude/rules/09-content-validators.md) governs downloads.

| Concern | Files | Tests |
|---|---|---|
| Replica cache + SQLite store | `omnibus/Offline/Cache.swift`, `omnibus/Offline/OfflineStore.swift` | `omnibusTests/OfflineSyncTests.swift` |
| Mutation outbox (`OpKind`, drain) | `omnibus/Offline/SyncEngine.swift` | `omnibusTests/OfflineSyncTests.swift` |
| Whole-library local index | `omnibus/Offline/LibraryIndex.swift` | `omnibusTests/LibraryPollTests.swift` |
| Downloads (resume, staleness, verify) | `omnibus/Offline/DownloadManager.swift` | `omnibusTests/MultipartDownloadTests.swift` |
| Connectivity signal | `omnibus/Offline/Connectivity.swift` | — |
| Background refresh & lifecycle | `omnibus/Offline/LifecycleSync.swift`, `omnibus/App/RefreshTask.swift` | `omnibusTests/RefreshTaskTests.swift` |
| Cross-device sync prompt | `omnibus/Offline/SyncPromptStore.swift`, `omnibus/Design/Components/SyncOfferBanner.swift` | — |
| Position reconcile | `omnibus/Offline/PositionSync.swift` | `omnibusTests/PositionPushThrottleTests.swift` |

## Design system

| Concern | Files | Tests |
|---|---|---|
| Theme tokens, type scale, colour | `omnibus/Design/Theme.swift`, `omnibus/Design/OKLCH.swift`, `omnibus/Design/Appearance.swift` | `omnibusTests/ThemeFontTests.swift`, `omnibusTests/SymbolNameTests.swift` |
| Motion curves | `omnibus/Design/Motion.swift` | — |
| Shared components | `omnibus/Design/Components/` | `omnibusTests/ConfettiTests.swift` |

## Coverage gaps worth knowing

- **`omnibusUITests` is one file** — `ConnectSmokeTests`. The shell's chrome has
  no UI coverage; the keyboard-vs-tab-bar test was deleted as flaky, and rule
  [04](../.claude/rules/04-playwright.md) explains why a replacement must be a
  layout test in `omnibusTests`, not a simulator drive.
- **Never drive the on-screen keyboard through XCUITest** — it blows the
  framework's internal snapshot budget and no test-side timeout can extend it.
- **A UI-test failure message lives only inside the `.xcresult`**, not stdout or
  JUnit. Read it back with `xcrun xcresulttool`; see
  [01-dev-environment.md](../.claude/rules/01-dev-environment.md).
- **No iOS surface** for: log viewer, admin health, user management, library
  cleanup, OPDS, Kobo device management, or send-to-Kindle/Kobo. Those are
  web-only — see [feature-map-web.md](feature-map-web.md).
