# 5. Schema details worth copying (and improving)

## Calibre side — verbatim from [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py)

```python
class Books(Base):
    __tablename__ = 'books'
    DEFAULT_PUBDATE = datetime(101, 1, 1, 0, 0, 0, 0)
    id = Column(Integer, primary_key=True, autoincrement=True)
    title = Column(String(collation='NOCASE'), nullable=False, default='Unknown')
    sort = Column(String(collation='NOCASE'))
    author_sort = Column(String(collation='NOCASE'))
    timestamp = Column(TIMESTAMP, default=lambda: datetime.now(timezone.utc))
    pubdate = Column(TIMESTAMP, default=DEFAULT_PUBDATE)
    series_index = Column(String, nullable=False, default="1.0")   # stored as TEXT!
    last_modified = Column(TIMESTAMP, default=lambda: datetime.now(timezone.utc))
    path = Column(String, default="", nullable=False)
    has_cover = Column(Integer, default=0)
    uuid = Column(String)
```

```python
class Data(Base):
    __tablename__ = 'data'
    id = Column(Integer, primary_key=True)
    book = Column(Integer, ForeignKey('books.id'), nullable=False)
    format = Column(String(collation='NOCASE'), nullable=False)
    uncompressed_size = Column(Integer, nullable=False)
    name = Column(String, nullable=False)   # filename stem, no extension
```

## On-disk path construction

From `convert_book_format` in [cps/helper.py](https://github.com/janeczku/calibre-web/blob/master/cps/helper.py):

```python
file_path = os.path.join(calibre_path, book.path, data.name + "." + book_format.lower())
```

That is: `<library_root>/<books.path>/<data.name>.<format-lowercased>`. `books.path` is typically `Author/Title (id)`; `data.name` duplicates that title fragment without extension. Omnibus should keep this convention for Calibre interop but normalize internally — `data.name` is redundant with `books.path` 99% of the time. Store the extension in a separate column rather than a format-join.

## Collations and indices

Calibre uses SQLite `NOCASE` collation on every string column you'd ever search. sqlx doesn't expose collation declaratively — declare columns with `COLLATE NOCASE` in the CREATE TABLE string.

**Note: no explicit `Index()` declarations** anywhere in [cps/db.py](https://github.com/janeczku/calibre-web/blob/master/cps/db.py). Calibre-desktop adds some indices (`idx_books_timestamp`, `authors_idx`) directly on disk but Calibre-Web ORM-side declarations don't. Omnibus should add indices on `(timestamp)`, `(last_modified)`, `(sort)`, `(author_sort)`, `(series_index)` plus each link table's reverse column. An index on `books.uuid` is also needed for Kobo.

## Link tables

Six simple m2m (`books_authors_link`, `books_tags_link`, `books_series_link`, `books_ratings_link`, `books_languages_link`, `books_publishers_link`), all with compound primary keys `(book, <entity>)` and no other columns.

## `custom_columns` table

Row shape: `(id, label, name, datatype, mark_for_delete, editable, display, is_multiple, normalized)`. At startup, Calibre-Web executes `SELECT id, datatype FROM custom_columns` and dynamically **materializes** a SQLAlchemy class per row:

```python
cc = conn.execute(text("SELECT id, datatype FROM custom_columns"))
cls.setup_db_cc_classes(cc)
setattr(Books, 'custom_column_' + str(cc_id[0]), relationship(...))
```

The backing tables are `custom_column_N` (and `books_custom_column_N_link` for multi-valued ones). Omnibus likely wants a single `custom_columns`/`custom_column_values` table with a `datatype` discriminator rather than DDL-per-column — Calibre's approach is an artifact of the desktop app predating JSON/discriminated storage.

## `app.db` (user/shelf side) — [cps/ub.py](https://github.com/janeczku/calibre-web/blob/master/cps/ub.py)

```python
class User(UserBase, Base):
    __tablename__ = 'user'
    id = Column(Integer, primary_key=True)
    name = Column(String(64), unique=True)
    email = Column(String(120), unique=True, default="")
    role = Column(SmallInteger, default=constants.ROLE_USER)   # bitmask
    password = Column(String)
    kindle_mail = Column(String(120), default="")
    locale = Column(String(2), default="en")
    sidebar_view = Column(Integer, default=1)                  # bitmask
    default_language = Column(String(3), default="all")
    denied_tags = Column(String, default="")                   # comma-separated
    allowed_tags = Column(String, default="")
    denied_column_value = Column(String, default="")
    allowed_column_value = Column(String, default="")
    view_settings = Column(JSON, default={})
    kobo_only_shelves_sync = Column(Integer, default=0)
```

Other tables: `Shelf`, `ReadBook`, `Bookmark`, `KoboReadingState`, `KoboStatistics`, `KoboBookmark`, `KoboSyncedBooks`, `ArchivedBook`, `Downloads`, `RemoteAuthToken`, `Registration`, `User_Sessions`.

## "Migrations" = runtime ALTER TABLE

[cps/config_sql.py](https://github.com/janeczku/calibre-web/blob/master/cps/config_sql.py) does **not use Alembic**. `_migrate_table(session, orm_class, secret_key=None)` iterates each ORM column, tries a `SELECT`, catches `OperationalError`, and synthesizes `ALTER TABLE ADD COLUMN` with defaults. No version table, no down-migrations. For Omnibus, keep the `initialize_schema` inline approach today but plan for `refinery`/`sqlx-migrate` once the schema stabilizes.

---

[← Dioxus / Rust wins](4-dioxus-rust-wins.md) · [Next: API surface →](6-api-surface.md)
