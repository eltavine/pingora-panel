-- Policies that decide which changes need approval before they are applied.
CREATE TABLE approval_policies (
    id TEXT PRIMARY KEY,
    policy TEXT NOT NULL CHECK (json_valid(policy)),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- Requests to apply content that policies cover. Expiry and content that
-- moved on are judged when a request is read; the stored state records what
-- people did.
CREATE TABLE approval_requests (
    id BLOB PRIMARY KEY CHECK (length(id) = 16),
    state TEXT NOT NULL
        CHECK (state IN ('pending', 'approved', 'rejected', 'withdrawn', 'outdated', 'applied')),
    draft_version INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    note TEXT,
    risk TEXT NOT NULL,
    policies TEXT NOT NULL CHECK (json_valid(policies)),
    required INTEGER NOT NULL CHECK (required > 0),
    valid_minutes INTEGER NOT NULL CHECK (valid_minutes > 0),
    changes TEXT NOT NULL CHECK (json_valid(changes)),
    closed_by TEXT,
    closed_at TEXT,
    reason TEXT,
    revision INTEGER
) STRICT;

CREATE INDEX approval_requests_newest ON approval_requests (requested_at DESC);
CREATE INDEX approval_requests_open ON approval_requests (content_hash)
    WHERE state IN ('pending', 'approved');

-- One approval per person and request; revoking keeps the row.
CREATE TABLE approvals (
    request_id BLOB NOT NULL REFERENCES approval_requests (id) ON DELETE CASCADE,
    approver TEXT NOT NULL,
    approved_at TEXT NOT NULL,
    valid_until TEXT NOT NULL,
    revoked_at TEXT,
    PRIMARY KEY (request_id, approver)
) STRICT, WITHOUT ROWID;
