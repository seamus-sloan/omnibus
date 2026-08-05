# Send a book to your Kobo

Omnibus can hand a book to a Kobo e-reader as a **KEPUB** — Kobo's own EPUB
variant, which gets noticeably faster page turns than a plain EPUB. It's a
**wired transfer**: your Kobo plugs into the computer running the browser (never
the Omnibus server), and the book copies onto it over USB.

On **Chrome or Edge**, Omnibus writes the file straight onto the plugged-in
device in one click. On other browsers (Firefox, Safari), it downloads the KEPUB
and you copy it over yourself. Both are described below.

> [!TIP]
> This transfer is completely safe for your device. Copying a file over USB
> **cannot delete or change anything already on your Kobo** — not your books,
> not your highlights, not your notes. Omnibus only ever *adds* a book file; it
> never touches the device's internal database. (That is *not* true of wireless
> sync, a separate and still-incomplete feature — see
> [About wireless sync](#about-wireless-sync-experimental).)

## Chrome / Edge — one-click write

1. Connect your Kobo with a USB cable and tap **Connect** on the device. It
   appears as a USB drive.
2. Open the book's page in Omnibus and click **Send to Kobo**.
3. The first time, your browser asks you to pick a folder — choose the **Kobo
   drive** and allow saving. Omnibus remembers it, so later sends write silently
   with no prompt.
4. When it reports success, **eject the Kobo safely**, then unplug it. The book
   appears in your library, titled from its own metadata (not the filename).

On this one-click write path, Omnibus files each book under `<Author>/<Title>/`
on the drive (the same tidy layout Calibre uses), so browsing the device over
USB stays organized rather than a pile of files at the root. (The download-then-
copy path below hands you a single file instead — drop it wherever you like.)

## Other browsers — download, then copy

1. Open the book's page in Omnibus and click **Send to Kobo**. A file named
   `<id>.kepub.epub` downloads to your computer.
2. Connect your Kobo with a USB cable and tap **Connect** on the device. It
   appears as a USB drive.
3. Copy the downloaded `.kepub.epub` file onto the Kobo drive. You can drop it at
   the top level or into any folder — the Kobo scans the whole drive.
4. **Eject the Kobo safely**, then unplug it. The book appears in your library.

Either way, open the book on the Kobo and you'll get the faster KEPUB page turns.

## What this does and doesn't do

- **Does:** put a reading-optimized copy of the book on your device.
- **Doesn't:** sync your reading position, mark the book finished, or send
  anything back to Omnibus. It's a one-way file copy.
- **Doesn't touch existing device content.** Nothing on the Kobo is modified or
  deleted by copying a file to it.

## If the download is a plain `.epub`

Omnibus converts to KEPUB with a tool called
[`kepubify`](https://github.com/pgaskin/kepubify). If that tool isn't installed
on the server, Omnibus falls back to serving the **plain EPUB** (filename ends in
`.epub` instead of `.kepub.epub`). A Kobo reads plain EPUB fine — page turns are
just a bit slower. The official Docker image ships `kepubify`; if you build from
source, install `kepubify` on the server's `PATH` (or set `OMNIBUS_KEPUBIFY_PATH`)
to get KEPUB output. See [.env.example](../.env.example).

## About wireless sync (experimental)

*Wireless* Kobo sync — where the device talks to Omnibus over Wi-Fi instead of a
USB cable — is functional but **experimental**: it has not yet passed
verification against real devices.

> [!WARNING]
> **First sync can erase your Kobo's highlights**
>
> An improper sync can potentially wipe data on the Kobo like your annotations,
> notes, bookmarks, reading progress, and books.
>
> It is highly recommended to back up your device before attempting a sync with
> one of these tools:
>
> - [seamus-sloan/kobo-backup](https://github.com/seamus-sloan/kobo-backup#kobo-backup)
> - [karlicoss/kobuddy](https://github.com/karlicoss/kobuddy#usage)
>
> At minimum, copy `.kobo/KoboReader.sqlite` off the device over USB.

### Setting up wireless sync

From **Account → Kobo wireless sync**:

1. Give your Kobo a name and click **Add a Kobo**
2. Copy the device's wireless sync endpoint URL. (`/kobo/<token>`)
3. Connect the Kobo over USB,
4. Edit `.kobo/Kobo/Kobo eReader.conf` and set `api_endpoint=` under
   `[OneStoreServices]` to `<your_omnibus_server_url>/kobo/<token>`.
5. Eject safely. Your next sync on the device talks to Omnibus.

(`.kobo` is a hidden folder — on macOS press
<kbd>⌘</kbd><kbd>⇧</kbd><kbd>.</kbd> in Finder to reveal it; on Windows enable
**Hidden items** in Explorer's View tab.)

Only shelves you've opted in are synced — mark a shelf **Sync to Kobo** in its
settings to include it.

### Highlights and notes

Highlights sync both ways. Highlights made **on the device** flow up to
Omnibus automatically and appear in the web reader. Highlights created **in
the web reader** are converted into device-placeable anchors and delivered to
the Kobo on its next sync — recolors, note edits, and deletes follow.

A web highlight is anchored against the book's converted KEPUB copy, so that
copy has to exist before the highlight can be placed. It is built on demand
and cached; when it is missing, Omnibus queues the conversion itself and
materializes the pending highlights as soon as it lands. Nothing is lost in
the meantime — the highlight stays web-only until the anchor can be derived.

Two things can stop that: `kepubify` not being installed (see [If the
download is a plain `.epub`](#if-the-download-is-a-plain-epub)), and the
book's source file being unreachable when the conversion runs — a library on
a network share that isn't mounted, for example. Both retry on the next
highlight you make on that book, and on every server restart.

Even with the KEPUB in hand, a highlight only converts when it and the source
EPUB still carry the same text — a conversion that can't be proven correct is
skipped (the highlight simply stays web-only) rather than placed somewhere
wrong.

> [!NOTE]
> **Annotations only materialise on a book the device actually holds**
>
> Highlights for a book are only useful once the device has that book's file —
> so Omnibus only lets a wireless GET for a book's annotations count as
> delivered once the device has downloaded it (over this same wireless
> channel). Downloading a book always triggers an unconditional fetch of its
> annotations, independent of the periodic sync check — so opening (or
> re-opening) **Reading → Sync now** on the device after downloading a book is
> the reliable way to pull down highlights that were waiting for it.

> [!NOTE]
> **Highlight colour on-wire shape: changed to a firmware hex swatch, still
> unconfirmed against a real device**
>
> Omnibus used to send each annotation's colour as `highlightColor` — a
> lowercase CSS-style colour name (`yellow`/`green`/`blue`/`pink`/`purple`)
> — in the Reading Services annotation payload. On at least one real device,
> every annotation pushed from Omnibus was observed stored on-device with
> `Bookmark.Color = 0` (yellow) regardless of the colour sent, while
> highlights created directly on the device use the full `Color` range
> normally (the original report, with the device's `KoboReader.sqlite` data:
> [#1629](https://github.com/seamus-sloan/omnibus/issues/1629)).
>
> This repo still carries no captured device PATCH for the Reading Services
> protocol, but a follow-up investigation found a genuine shape difference
> against [bookorbit](https://github.com/bookorbit/bookorbit) — an
> actively-developed, open-source Kobo sync project whose own Reading
> Services client (`kobo-annotation-payload.ts` /
> `annotation-style-map.ts`) sends `highlightColor` as one of **four fixed
> hex swatches**, explicitly commented "Kobo firmware's four fixed highlight
> colors":
>
> | Named colour | Hex swatch |
> |---|---|
> | yellow | `#F6F3B3` |
> | green | `#C6E09E` |
> | blue | `#B2E1E8` |
> | pink | `#E8AFCF` |
>
> A CSS-style name being silently ignored in favour of the palette default
> would exactly reproduce the observed symptom — every pushed annotation
> landing at index 0 (yellow) — so Omnibus's outbound `highlightColor` now
> sends these hex values (`color_to_kobo` /
> `color_from_kobo` in
> [reading_services/dto.rs](../server/src/backend/kobo/reading_services/dto.rs)).
> The inbound parser accepts **both** the hex swatches and the old CSS names,
> so an already-adopted device or an unverified firmware revision that still
> sends a name keeps working.
>
> **This is still unverified against real hardware** — bookorbit's own
> source carries no comment or test citing a device capture for this exact
> field either, so this fix is corroborated by a comparable open-source
> implementation, not proven. If highlights pushed from Omnibus still land
> as yellow after this change, the firmware likely ignores `highlightColor`
> on inbound annotations entirely rather than wanting a different shape, and
> that would be the next thing to rule out. Kobo's classic highlight menu
> has no fifth swatch for Omnibus's violet, so on-device it renders
> identically to rose (pink) — a real palette limit, not a bug. Highlight
> and note **text**, placement, and deletes were never affected — only
> colour. Confirming (or refuting) this needs a `.kobo/KoboReader.sqlite`
> capture (or a packet capture) from a device that just received a coloured
> highlight over wireless sync, per AC2 of #1629.
