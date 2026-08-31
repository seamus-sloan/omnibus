-- Display suffix for API tokens: the last 4 characters of the raw token,
-- captured at creation so the management UI can render a recognizable
-- `omni_…xxxx` identifier without ever storing the secret itself. Four
-- trailing characters of a 32-byte-random credential reveal nothing usable.
-- Nullable: rows minted before this migration have no recorded suffix and
-- render without one (never faked from the hash).
ALTER TABLE api_tokens ADD COLUMN suffix TEXT;
