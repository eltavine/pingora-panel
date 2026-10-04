-- Every configuration that was applied or attempted. A revision's files never
-- change; its note and outcome do.
CREATE TABLE configuration_revisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    draft_version INTEGER NOT NULL CHECK (draft_version > 0),
    language_version INTEGER NOT NULL,
    sources TEXT NOT NULL CHECK (json_valid(sources)),
    content_hash TEXT NOT NULL,
    author TEXT NOT NULL,
    note TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    outcome TEXT NOT NULL
        CHECK (outcome IN ('applying', 'active', 'superseded', 'rejected', 'failed')),
    outcome_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    -- Why a rejected or failed attempt did not run.
    diagnostics TEXT CHECK (diagnostics IS NULL OR json_valid(diagnostics)),
    -- The runtime snapshot the gateway activated.
    snapshot_hash TEXT,
    gateway_revision INTEGER
) STRICT;

CREATE UNIQUE INDEX configuration_revisions_one_active
    ON configuration_revisions (outcome) WHERE outcome = 'active';
