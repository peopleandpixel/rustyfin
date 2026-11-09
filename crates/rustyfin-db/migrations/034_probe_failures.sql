-- Files whose probe failed, with their size and modification time when it failed, so that scans
-- do not probe them again until they change.
CREATE TABLE probe_failures (
    path TEXT PRIMARY KEY NOT NULL,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    failed_at TEXT NOT NULL
);
