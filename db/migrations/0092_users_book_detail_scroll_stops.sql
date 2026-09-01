-- Per-user "use book detail scroll stops" preference.
--
-- 0 (the default) renders the book detail page as one continuous scroll; 1
-- opts into the snap-stop marquee both clients ship today. Existing rows take
-- the default, so the setting arrives off for everyone.
ALTER TABLE users ADD COLUMN book_detail_scroll_stops INTEGER NOT NULL DEFAULT 0;
