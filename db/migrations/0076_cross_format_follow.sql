-- Follow mode + user-declared sync points on the per-user link record.
-- `follow` switches the link from offer-based prompts to resolve-at-read
-- (every surface resumes at the freshest position across both formats).
-- `user_anchors` stores "synced here" declarations as a JSON array of
-- [ebook_fraction, audio_global_seconds] pairs; they outrank chapter
-- anchors in the mapping and re-declaring nearby replaces the old pair.
ALTER TABLE cross_format_links ADD COLUMN follow INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cross_format_links ADD COLUMN user_anchors TEXT NOT NULL DEFAULT '[]';
