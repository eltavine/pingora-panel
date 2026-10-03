-- Policies that decide which changes need approval before they are applied.
CREATE TABLE approval_policies (
    id text PRIMARY KEY,
    policy jsonb NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

-- Requests to apply content that policies cover. Expiry and content that
-- moved on are judged when a request is read; the stored state records what
-- people did.
CREATE TABLE approval_requests (
    id uuid PRIMARY KEY,
    state text NOT NULL
        CHECK (state IN ('pending', 'approved', 'rejected', 'withdrawn', 'outdated', 'applied')),
    draft_version bigint NOT NULL,
    content_hash text NOT NULL,
    requested_by text NOT NULL,
    requested_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    note text,
    risk text NOT NULL,
    policies jsonb NOT NULL,
    required integer NOT NULL CHECK (required > 0),
    valid_minutes integer NOT NULL CHECK (valid_minutes > 0),
    changes jsonb NOT NULL,
    closed_by text,
    closed_at timestamptz,
    reason text,
    revision bigint
);

CREATE INDEX approval_requests_newest ON approval_requests (requested_at DESC);
CREATE INDEX approval_requests_open ON approval_requests (content_hash)
    WHERE state IN ('pending', 'approved');

-- One approval per person and request; revoking keeps the row.
CREATE TABLE approvals (
    request_id uuid NOT NULL REFERENCES approval_requests (id) ON DELETE CASCADE,
    approver text NOT NULL,
    approved_at timestamptz NOT NULL,
    valid_until timestamptz NOT NULL,
    revoked_at timestamptz,
    PRIMARY KEY (request_id, approver)
);
