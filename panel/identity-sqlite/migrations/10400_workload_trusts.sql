-- Trusts in short-lived tokens from an issuer, exchanged for sessions of a
-- service account.
CREATE TABLE workload_trusts (
    id TEXT PRIMARY KEY,
    account_id BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    audience TEXT NOT NULL,
    subject TEXT NOT NULL,
    claims TEXT NOT NULL CHECK (json_valid(claims)),
    session_minutes INTEGER NOT NULL CHECK (session_minutes BETWEEN 5 AND 60),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE INDEX workload_trusts_account ON workload_trusts (account_id);
