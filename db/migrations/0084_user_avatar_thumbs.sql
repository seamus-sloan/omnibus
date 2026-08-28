-- Store a downscaled avatar alongside the original upload (#2245).
--
-- The avatar endpoint returned the untouched upload — observed at 1446x2200,
-- ~600 KB — to fill a 28px nav square, on every page that renders the nav,
-- with a 10 MB upload cap behind it. `thumb_bytes` is what the nav is served
-- now; the original stays for any surface that wants a larger rendering.
--
-- Nullable rather than backfilled in SQL: the value is an image encode, which
-- SQL can't compute. `backfill_avatar_thumbs` fills existing rows at boot and
-- is a no-op once caught up, and a NULL thumb serves the original meanwhile —
-- so an avatar is never missing, only briefly larger than it needs to be.
ALTER TABLE user_avatars ADD COLUMN thumb_mime TEXT;
ALTER TABLE user_avatars ADD COLUMN thumb_bytes BLOB;
