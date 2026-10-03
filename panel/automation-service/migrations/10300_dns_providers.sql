-- DNS providers that publish DNS-01 records. Settings are public; secrets
-- are sealed with the deployment's master keys.
CREATE TABLE dns_providers (
    provider_id text PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('rfc2136')),
    settings jsonb NOT NULL,
    sealed_secret text NOT NULL,
    -- Seconds to wait for a record to reach every authoritative server.
    propagation_seconds integer NOT NULL CHECK (propagation_seconds BETWEEN 0 AND 3600),
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

-- DNS-01 certificates name the provider that publishes their records.
ALTER TABLE acme_certificates
    ADD COLUMN dns_provider_id text REFERENCES dns_providers (provider_id),
    ADD CONSTRAINT acme_certificates_dns_provider
        CHECK ((challenge = 'dns-01') = (dns_provider_id IS NOT NULL));
