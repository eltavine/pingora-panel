-- Trusts in short-lived tokens from an issuer, exchanged for sessions of a
-- service account.
CREATE TABLE workload_trusts (
    id text PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    issuer text NOT NULL,
    audience text NOT NULL,
    subject text NOT NULL,
    claims jsonb NOT NULL,
    session_minutes integer NOT NULL CHECK (session_minutes BETWEEN 5 AND 60),
    enabled boolean NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE INDEX workload_trusts_account ON workload_trusts (account_id);
