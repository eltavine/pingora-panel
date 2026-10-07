-- What administrators decided about each plugin. Versions themselves live in
-- the plugins directory, where they are found and checked again.
CREATE TABLE plugins (
    name TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    active_version TEXT,
    -- The version a rollback returns to.
    previous_version TEXT,
    -- A JSON array of granted capabilities.
    grants TEXT NOT NULL CHECK (json_type(grants) = 'array'),
    -- The settings as written; secret references stay references.
    settings TEXT NOT NULL CHECK (json_type(settings) = 'object'),
    limits TEXT NOT NULL CHECK (json_type(limits) = 'object'),
    version INTEGER NOT NULL CHECK (version > 0),
    updated_at TEXT NOT NULL,
    CHECK (enabled = 0 OR active_version IS NOT NULL)
) STRICT;

-- Publisher keys whose minisign signatures the host accepts.
CREATE TABLE trusted_keys (
    id TEXT PRIMARY KEY,
    key_id TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    comment TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

-- Secrets that plugins' settings name as `vault:<name>`, sealed with the
-- deployment's master keys.
CREATE TABLE secrets (
    name TEXT PRIMARY KEY,
    sealed TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
