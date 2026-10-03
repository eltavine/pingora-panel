CREATE TABLE identity_providers (
    id text PRIMARY KEY,
    display_name text NOT NULL,
    issuer text NOT NULL,
    client_id text NOT NULL,
    -- Sealed under the deployment's master keys.
    client_secret text,
    scopes text[] NOT NULL DEFAULT '{}',
    claims jsonb NOT NULL,
    group_roles jsonb NOT NULL DEFAULT '[]',
    create_accounts boolean NOT NULL,
    enabled boolean NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE TABLE provider_links (
    provider_id text NOT NULL REFERENCES identity_providers (id) ON DELETE CASCADE,
    subject text NOT NULL,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- Roles the provider's group mappings added to the account.
    granted_roles text[] NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL,
    PRIMARY KEY (provider_id, subject)
);

CREATE INDEX provider_links_account ON provider_links (account_id);

CREATE TABLE pending_sign_ins (
    state_hash bytea PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    provider_id text NOT NULL REFERENCES identity_providers (id) ON DELETE CASCADE,
    nonce text NOT NULL,
    verifier text NOT NULL,
    return_to text NOT NULL,
    expires_at timestamptz NOT NULL
);

CREATE INDEX pending_sign_ins_expiry ON pending_sign_ins (expires_at);

CREATE TABLE provider_sessions (
    session_id uuid PRIMARY KEY REFERENCES sessions (id) ON DELETE CASCADE,
    provider_id text NOT NULL REFERENCES identity_providers (id) ON DELETE CASCADE,
    -- The provider's refresh token, sealed.
    refresh_token text,
    checked_at timestamptz NOT NULL
);
