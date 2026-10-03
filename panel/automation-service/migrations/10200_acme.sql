-- ACME accounts. Credentials hold the account key and are sealed with the
-- deployment's master keys.
CREATE TABLE acme_accounts (
    account_id text PRIMARY KEY,
    directory_url text NOT NULL,
    -- PEM roots trusted for a private directory instead of the platform's.
    ca_bundle text,
    contact text[] NOT NULL,
    external_account_key_id text,
    account_url text NOT NULL,
    sealed_credentials text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

-- Certificates obtained and renewed through ACME, by the ID of the inventory
-- certificate each one produces.
CREATE TABLE acme_certificates (
    certificate_id text PRIMARY KEY,
    account_id text NOT NULL REFERENCES acme_accounts (account_id),
    names text[] NOT NULL,
    challenge text NOT NULL CHECK (challenge IN ('http-01', 'dns-01')),
    -- When the next issuance is due.
    renew_after timestamptz NOT NULL,
    -- When to ask the CA for its renewal window again.
    window_checked_after timestamptz,
    window_explanation_url text,
    -- Held by the job issuing the certificate.
    issuing_until timestamptz,
    failures integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    last_error_code text,
    last_error_message text,
    last_attempt_at timestamptz,
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE INDEX acme_certificates_renew_after ON acme_certificates (renew_after);

-- The smallest number of days before its end that a certificate's version
-- was announced as expiring at; 0 once it has expired.
ALTER TABLE certificates ADD COLUMN reminded_days integer;
