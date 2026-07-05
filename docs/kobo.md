# Send a book to your Kobo

Omnibus can hand a book to a Kobo e-reader as a **KEPUB** — Kobo's own EPUB
variant, which gets noticeably faster page turns than a plain EPUB. Today this
is a **manual, wired transfer**: you download the KEPUB from Omnibus and copy it
onto the Kobo over USB.

> [!TIP]
> This transfer is completely safe for your device. Copying a file over USB
> **cannot delete or change anything already on your Kobo** — not your books,
> not your highlights, not your notes. (That is *not* true of wireless "sync",
> which is a different feature Omnibus does not yet offer — see the note at the
> bottom.)

## Steps

1. Open the book's page in Omnibus and click **Send to Kobo**. A file named
   `<id>.kepub.epub` downloads to your computer.
2. Connect your Kobo to the computer with a USB cable and tap **Connect** on the
   device. It appears as a USB drive.
3. Copy the downloaded `.kepub.epub` file onto the Kobo drive. You can drop it at
   the top level or into any folder — the Kobo scans the whole drive.
4. **Eject the Kobo safely**, then unplug it. The book appears in your library,
   titled from its own metadata (not the filename).

That's it. Open the book on the Kobo and you'll get the faster KEPUB page turns.

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
