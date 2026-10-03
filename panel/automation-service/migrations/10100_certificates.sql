-- The certificate inventory. Chains are public; private keys are sealed
-- with the deployment's master keys and bound to their certificate.
CREATE TABLE certificates (
    certificate_id text PRIMARY KEY,
    source text NOT NULL CHECK (source IN ('uploaded', 'self_signed', 'acme')),
    details jsonb NOT NULL,
    chain text NOT NULL,
    sealed_key text NOT NULL,
    not_after timestamptz NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE INDEX certificates_not_after ON certificates (not_after);
