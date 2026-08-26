# site/ — the Omnibus marketing site

Static HTML served at <https://seamus-sloan.github.io/omnibus/> from the
`gh-pages` branch. No build step and no dependencies: `site/src/` is copied
verbatim by [`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on
every push to `main` that touches it.

```
src/
  index.html   twelve panels — all copy is real HTML, not baked into an image
  site.css     --pg-* page chrome tokens, two colour directions, flow fallback
  app.js       the panel deck (wheel/key/touch), direction toggle, section rail
  shots/       app screenshots, WebP
  assets/      the mascot
```

## The screenshots

The framed screens are **stills**, exported at 2x from the Claude Design
"Omnibus" project (file `Omnibus - Site.html`) and re-encoded by
[`scripts/site-shots.sh`](../scripts/site-shots.sh). The design project is
their source of truth — the PNGs are not committed, only the WebP output.

They are therefore a **point-in-time picture of the design, not of what the
server currently renders**. When a redesigned surface ships under
[#2132](https://github.com/seamus-sloan/omnibus/issues/2132), re-export the
affected screen and re-run the script. Replacing these with captures driven
against a live server is tracked separately.

Three frames are still placeholders (`.frame--todo` / `.phone--todo` in
`index.html`): the Kobo shelf editor, the metadata editor, and the check-in
success state.

## Working on it locally

```bash
python3 -m http.server 8000 --directory site/src   # then open localhost:8000
```

`app.js` engages the locked-panel deck only above 861x560; below that — and
with JavaScript off — `body.flow` leaves the panels as ordinary stacked
sections. Test both.
