-- The configuration document operators edit, and which version of it the
-- gateway runs.
CREATE TABLE draft_configuration (
    singleton INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version >= 0),
    format TEXT NOT NULL,
    document TEXT NOT NULL CHECK (json_valid(document)),
    -- The draft written in the configuration language, file by file, beside
    -- the model it describes; a draft without them is printed from its
    -- model when first read.
    sources TEXT CHECK (sources IS NULL OR json_valid(sources)),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    applied_version INTEGER,
    applied_at TEXT
) STRICT, WITHOUT ROWID;

-- The result of each change by idempotency key, so a retried change returns
-- its first result instead of applying twice.
CREATE TABLE change_receipts (
    idempotency_key TEXT PRIMARY KEY,
    operation TEXT NOT NULL,
    resource TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    content BLOB NOT NULL,
    etag TEXT NOT NULL,
    version INTEGER NOT NULL,
    recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
) STRICT;
