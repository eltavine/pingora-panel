-- DNS providers that publish DNS-01 records. Settings are public; secrets
-- are sealed with the deployment's master keys.
CREATE TABLE dns_providers (
    provider_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('rfc2136')),
    settings TEXT NOT NULL CHECK (json_valid(settings)),
    sealed_secret TEXT NOT NULL,
    -- Seconds to wait for a record to reach every authoritative server.
    propagation_seconds INTEGER NOT NULL CHECK (propagation_seconds BETWEEN 0 AND 3600),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- ACME accounts. Credentials hold the account key and are sealed with the
-- deployment's master keys.
CREATE TABLE acme_accounts (
    account_id TEXT PRIMARY KEY,
    directory_url TEXT NOT NULL,
    -- PEM roots trusted for a private directory instead of the platform's.
    ca_bundle TEXT,
    -- A JSON array of contact URIs.
    contact TEXT NOT NULL CHECK (json_type(contact) = 'array'),
    external_account_key_id TEXT,
    account_url TEXT NOT NULL,
    sealed_credentials TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- Certificates obtained and renewed through ACME, by the ID of the inventory
-- certificate each one produces.
CREATE TABLE acme_certificates (
    certificate_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES acme_accounts (account_id),
    -- A JSON array of the names the certificate covers.
    names TEXT NOT NULL CHECK (json_type(names) = 'array'),
    challenge TEXT NOT NULL CHECK (challenge IN ('http-01', 'dns-01')),
    -- DNS-01 certificates name the provider that publishes their records.
    dns_provider_id TEXT REFERENCES dns_providers (provider_id),
    -- When the next issuance is due.
    renew_after TEXT NOT NULL,
    -- When to ask the CA for its renewal window again.
    window_checked_after TEXT,
    window_explanation_url TEXT,
    -- Held by the job issuing the certificate.
    issuing_until TEXT,
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    last_error_code TEXT,
    last_error_message TEXT,
    last_attempt_at TEXT,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((challenge = 'dns-01') = (dns_provider_id IS NOT NULL))
) STRICT;

CREATE INDEX acme_certificates_renew_after ON acme_certificates (renew_after);
CREATE INDEX acme_certificates_account ON acme_certificates (account_id);
CREATE INDEX acme_certificates_dns_provider ON acme_certificates (dns_provider_id);
