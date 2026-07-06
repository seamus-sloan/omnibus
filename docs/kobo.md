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
> "sync", which is a different feature Omnibus does not yet offer — see the note
> at the bottom.)

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

## About wireless sync (not yet available)

Some servers offer *wireless* Kobo sync, where the device talks to the server
over Wi-Fi. Omnibus does **not** do this yet. It's worth knowing why the wired
copy above is the safe choice in the meantime: pointing a Kobo's wireless sync at
a server that doesn't fully implement Kobo's protocol can make the **device erase
its own highlights and notes**. The manual USB copy has no such risk. If wireless
sync is added later, Omnibus will warn you clearly before you enable it.
