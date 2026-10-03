-- The configuration document operators edit, and which version of it the
-- gateway runs.
CREATE TABLE draft_configuration (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    version bigint NOT NULL CHECK (version >= 0),
    format text NOT NULL,
    document jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    applied_version bigint,
    applied_at timestamptz
);

-- The result of each change by idempotency key, so a retried change returns
-- its first result instead of applying twice.
CREATE TABLE change_receipts (
    idempotency_key text PRIMARY KEY,
    operation text NOT NULL,
    resource text NOT NULL,
    request_hash text NOT NULL,
    content bytea NOT NULL,
    etag text NOT NULL,
    version bigint NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT now()
);
