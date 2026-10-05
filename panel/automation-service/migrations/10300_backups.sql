-- Backups (ADR 0035): what each was asked to hold, how far taking it got
-- and the archive it left in the data directory's `backups` directory.
CREATE TABLE backups (
    backup_id TEXT PRIMARY KEY,
    -- What it holds, a JSON array such as ["configuration", "sites"].
    contents TEXT NOT NULL CHECK (json_type(contents) = 'array'),
    -- The directory below the sites' directory it holds; empty for all.
    site_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed')),
    requested_by TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    finished_at TEXT,
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    sha256 TEXT NOT NULL DEFAULT '',
    files INTEGER NOT NULL DEFAULT 0 CHECK (files >= 0),
    failure_code TEXT,
    failure_message TEXT,
    product_version TEXT NOT NULL,
    -- Members the caller attached, a JSON object of hexadecimal content by
    -- path, kept only until the archive is written.
    attachments TEXT CHECK (attachments IS NULL OR json_type(attachments) = 'object')
) STRICT;

CREATE INDEX backups_newest_first ON backups (requested_at DESC, backup_id);
