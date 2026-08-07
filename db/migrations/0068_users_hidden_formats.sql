-- Per-user hidden-formats preference for the landing All Books view.
--
-- Comma-separated lowercase format tokens (e.g. "cbz,m4b"); NULL/empty means
-- nothing hidden. Normalization (trim/lowercase/dedupe/sort) happens in
-- `db::auth::set_hidden_formats` — the column stores only canonical values.
ALTER TABLE users ADD COLUMN hidden_formats TEXT;
