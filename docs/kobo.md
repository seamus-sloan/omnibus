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
USB cable — is functional but **experimental**: the protocol implementation is
complete, but it has not yet passed verification against real devices. Back up
first (below), then expect rough edges.

> [!WARNING]
> **The first wireless sync can erase your Kobo's highlights and notes.**
> A Kobo clears its own annotations when the server it syncs with mishandles
> Kobo's annotation channel. Omnibus answers that channel, but until real-device
> verification passes, assume a first sync can still wipe on-device annotations —
> and Omnibus does **not** store what the device erases.

**Back up your device before the first wireless sync.** Two tools cover this
properly:

- [**kobo-backup**](https://github.com/seamus-sloan/kobo-backup) — snapshots the
  entire device over USB into a verified zip (books, annotations, reading
  progress, settings, databases) and can restore the Kobo to exactly that point
  in time. The most complete option: a wipe becomes a non-event.
- [**kobuddy**](https://github.com/karlicoss/kobuddy) — exports your highlights,
  annotations, and reading history from the device database into plain data you
  keep.

At minimum, copy the device database off by hand:

1. Connect the Kobo over USB and tap **Connect**. It appears as a USB drive.
2. Copy `.kobo/KoboReader.sqlite` off the drive to somewhere safe. (`.kobo` is a
   hidden folder — on macOS press <kbd>⌘</kbd><kbd>⇧</kbd><kbd>.</kbd> in Finder
   to reveal it; on Windows enable **Hidden items** in Explorer's View tab.)
3. Eject the Kobo safely.

That file holds every highlight and note on the device. Keeping a copy means a
wipe is recoverable.

### Setting up wireless sync

There is **no on-device setting** for a custom sync server — the endpoint is set
by editing a config file on the Kobo over USB:

1. In Omnibus, open **Account → Kobo wireless sync**, name your device, and
   click **Add a Kobo**. Omnibus mints it a private sync URL
   (`https://<your-server>/kobo/<token>`); the token authorizes only your
   library, and **Regenerate** revokes it.
2. Copy the device's **wireless sync endpoint** URL from the card.
3. Connect the Kobo over USB and tap **Connect**. Open the hidden
   `.kobo/Kobo/Kobo eReader.conf` file in a text editor.
4. In the `[OneStoreServices]` section, set `api_endpoint=` to the copied URL:

   ```ini
   [OneStoreServices]
   api_endpoint=https://your-server.example.com/kobo/<token>
   ```

5. Save, eject the Kobo safely, and unplug. The next sync on the device
   (**Sync** from the home screen) talks to Omnibus: books from shelves marked
   **Sync to Kobo** appear on the device, and reading position, read status,
   and highlights round-trip back.

Only shelves you've opted in are synced — mark a shelf **Sync to Kobo** in its
settings to include it.

The wired **Send to Kobo** transfer described above carries none of this risk and
stays the safe choice for simply putting books on the device.
