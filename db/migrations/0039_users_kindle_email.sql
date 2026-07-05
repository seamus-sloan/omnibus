-- F4.3: Send-to-Kindle. Per-user destination address the "Send to Kindle"
-- action emails the EPUB to. Nullable — a user opts in by setting it on their
-- account page; a NULL means the feature is unconfigured for that user and the
-- book-detail action stays disabled. No backfill needed.
ALTER TABLE users ADD COLUMN kindle_email TEXT;
