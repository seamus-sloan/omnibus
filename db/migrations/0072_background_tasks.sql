-- Durable background-task history (#941): the F0.5 worker's task state was
-- in-memory only (`Worker::progress` in db/src/worker/types.rs), so history
-- was lost on restart and an operator had no way to see what ran. This table
-- is the durable companion the worker writes to as tasks run — mirrors how
-- `error_ring` (fast, in-memory) pairs with the on-disk `logs` sink.
--
-- `task_kind` is free-text (the worker's own per-variant label, e.g.
-- "scan"/"kepub_convert" — finer-grained than the wire-facing `TaskKind` enum
-- that deliberately collapses several `Task` variants together for the live
-- progress UI). `status` starts `running` on insert and is updated in place
-- once the task finishes; `finished_at`/`error` stay NULL until then.
CREATE TABLE background_tasks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_kind   TEXT    NOT NULL,
    status      TEXT    NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    error       TEXT
);

-- The admin dashboard's only read pattern is "most recent N rows"; id order
-- already matches started_at order (autoincrement, single-writer inserts),
-- but an explicit index keeps a future started_at-range query cheap too.
CREATE INDEX idx_background_tasks_started_at ON background_tasks(started_at);
