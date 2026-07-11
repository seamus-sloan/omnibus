-- Draft support for journal entries (F3.2b). `status` distinguishes a
-- per-user-private draft from a published entry; existing rows default to
-- 'published' so nothing disappears from the shared feed on upgrade. The
-- feed query filters to published rows plus the viewer's own drafts.
ALTER TABLE journal_entries ADD COLUMN status TEXT NOT NULL DEFAULT 'published'
    CHECK (status IN ('draft', 'published'));
