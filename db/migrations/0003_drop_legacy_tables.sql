-- F0.1: Drop the renamed legacy tables now that migration 0002 and the
-- rewritten db layer have cut over to the normalized schema. Kept as a
-- separate migration so a rollback-before-deploy is reversible by simply
-- rewinding to migration 0002.

DROP TABLE IF EXISTS book_covers_legacy;
DROP TABLE IF EXISTS library_index_state_legacy;
DROP TABLE IF EXISTS books_legacy;
