# 6. API surface

- **OPDS 1.2** — 30+ endpoints under `/opds`. Atom XML. OpenSearch discovery at `/opds/osd`.
- **Kobo** — under `/kobo/v1/`: `library/sync`, `library/<uuid>/metadata`, `library/<uuid>/state`, `library/<uuid>` (DELETE), `library/tags`, `library/tags/<id>`, `library/tags/<id>/items`, `library/tags/<id>/items/delete`, `<uuid>/<w>/<h>/<grey>/image.jpg`, `download/<book_id>/<format>`. Auth wrapper `@requires_kobo_auth`. Sync token maintenance in [cps/services/SyncToken.py](https://github.com/janeczku/calibre-web/blob/master/cps/services/SyncToken.py).
- **JSON / AJAX** — there is **no general-purpose REST API**. Ad-hoc JSON endpoints are scattered: `/ajax/book/<uuid>` (Calibre Companion), `/ajax/listusers`, `/ajax/deleteuser`, `/ajax/log/<type>`, `/ajax/canceltask`, `/ajax/fullsync/<userid>`, `/ajax/verify_token`, `/opds/stats`. No single documented contract for a mobile client.

Omnibus' plan to expose `/api/*` as a first-class hand-written REST router (see [server/src/backend.rs](../../server/src/backend.rs)) is a real improvement; build it as the primary surface and layer OPDS + Kobo on top.

---

[← Schema details](5-schema-details.md) · [Next: recommendations →](7-recommendations.md)
