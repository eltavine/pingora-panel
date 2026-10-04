-- The certificate inventory. Chains are public; private keys are sealed
-- with the deployment's master keys and bound to their certificate.
CREATE TABLE certificates (
    certificate_id TEXT PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('uploaded', 'self_signed', 'acme')),
    -- What the chain says about itself, as JSON text.
    details TEXT NOT NULL CHECK (json_valid(details)),
    chain TEXT NOT NULL,
    sealed_key TEXT NOT NULL,
    not_after TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    -- The smallest number of days before its end that a certificate's
    -- version was announced as expiring at; 0 once it has expired.
    reminded_days INTEGER
) STRICT;

CREATE INDEX certificates_not_after ON certificates (not_after);
