-- Per-user Wishlist shelf names: "Wishlist" -> "<username>'s Wishlist".
--
-- Wishlists are public, so a browsing user sees every other user's shelf in one
-- rail; a shared literal name gave no way to tell them apart. Migration 0047
-- seeded them all as 'Wishlist' — rename in place so existing rows match what
-- `provision_wishlist_shelf` now composes at insert time.
--
-- Usernames are immutable (no rename path exists), so this runs once and the
-- name can't drift afterwards. `idx_shelves_owner_name` excludes the wishlist
-- kind, so the new name can't collide with a user's own shelf.

UPDATE shelves
   SET name = (SELECT u.username FROM users u WHERE u.id = shelves.owner_user_id) || '''s Wishlist',
       updated_at = strftime('%s','now')
 WHERE kind = 'wishlist'
   -- `name` is NOT NULL: skip any orphan row rather than fail the migration.
   AND EXISTS (SELECT 1 FROM users u WHERE u.id = shelves.owner_user_id);
