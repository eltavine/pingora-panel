-- DNS-01 certificates may name a plugin whose DNS-01 port publishes their
-- records (ADR 0044), in place of a DNS provider. SQLite changes a
-- constraint by rebuilding the table, with foreign key enforcement off.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE acme_certificates_next (
    certificate_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES acme_accounts (account_id),
    -- A JSON array of the names the certificate covers.
    names TEXT NOT NULL CHECK (json_type(names) = 'array'),
    challenge TEXT NOT NULL CHECK (challenge IN ('http-01', 'dns-01')),
    -- DNS-01 certificates name the provider or the plugin that publishes
    -- their records.
    dns_provider_id TEXT REFERENCES dns_providers (provider_id),
    dns_plugin TEXT,
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
    CHECK (dns_provider_id IS NULL OR dns_plugin IS NULL),
    CHECK ((challenge = 'dns-01') = (dns_provider_id IS NOT NULL OR dns_plugin IS NOT NULL))
) STRICT;

INSERT INTO acme_certificates_next (
    certificate_id, account_id, names, challenge, dns_provider_id, renew_after,
    window_checked_after, window_explanation_url, issuing_until, failures, last_error_code,
    last_error_message, last_attempt_at, version, created_at, updated_at
)
SELECT
    certificate_id, account_id, names, challenge, dns_provider_id, renew_after,
    window_checked_after, window_explanation_url, issuing_until, failures, last_error_code,
    last_error_message, last_attempt_at, version, created_at, updated_at
FROM acme_certificates;

DROP TABLE acme_certificates;
ALTER TABLE acme_certificates_next RENAME TO acme_certificates;

CREATE INDEX acme_certificates_renew_after ON acme_certificates (renew_after);
CREATE INDEX acme_certificates_account ON acme_certificates (account_id);
CREATE INDEX acme_certificates_dns_provider ON acme_certificates (dns_provider_id);

COMMIT;
PRAGMA foreign_keys = ON;
