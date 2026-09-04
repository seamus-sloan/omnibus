-- Creators set through metadata overrides never got an `authors` row
-- (#2235, #2343): every read resolves override creators by name, so a name
-- that lived only in override JSON had no id, no author page and no
-- `/api/authors` entry — while the file's scanned name kept a row nothing
-- displays. The override write paths now materialize the row
-- (`materialize_author_rows`); this heals the libraries written before they
-- did. Idempotent: `OR IGNORE` against the NOCASE-unique name.
INSERT OR IGNORE INTO authors (name, sort)
SELECT DISTINCT trim(json_extract(je.value, '$.name')),
       nullif(trim(coalesce(json_extract(je.value, '$.file_as'), '')), '')
  FROM metadata_overrides mo
  JOIN books b ON b.uuid = mo.book_uuid
  JOIN json_each(mo.overrides, '$.creators') je
 WHERE json_type(mo.overrides, '$.creators') IS NOT NULL
   AND trim(coalesce(json_extract(je.value, '$.name'), '')) <> '';
