CREATE TABLE identity_providers (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    -- Sealed under the deployment's master keys.
    client_secret TEXT,
    -- A JSON array of the scopes requested.
    scopes TEXT NOT NULL DEFAULT '[]' CHECK (json_type(scopes) = 'array'),
    claims TEXT NOT NULL CHECK (json_valid(claims)),
    group_roles TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(group_roles)),
    create_accounts INTEGER NOT NULL CHECK (create_accounts IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE provider_links (
    provider_id TEXT NOT NULL REFERENCES identity_providers (id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    account_id BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- Roles the provider's group mappings added to the account, as a JSON
    -- array.
    granted_roles TEXT NOT NULL DEFAULT '[]' CHECK (json_type(granted_roles) = 'array'),
    created_at TEXT NOT NULL,
    PRIMARY KEY (provider_id, subject)
) STRICT, WITHOUT ROWID;

CREATE INDEX provider_links_account ON provider_links (account_id);

CREATE TABLE pending_sign_ins (
    state_hash BLOB PRIMARY KEY CHECK (length(state_hash) = 32),
    provider_id TEXT NOT NULL REFERENCES identity_providers (id) ON DELETE CASCADE,
    nonce TEXT NOT NULL,
    verifier TEXT NOT NULL,
    return_to TEXT NOT NULL,
    expires_at TEXT NOT NULL
) STRICT;

CREATE INDEX pending_sign_ins_expiry ON pending_sign_ins (expires_at);
CREATE INDEX pending_sign_ins_provider ON pending_sign_ins (provider_id);

CREATE TABLE provider_sessions (
    session_id BLOB PRIMARY KEY REFERENCES sessions (id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL REFERENCES identity_providers (id) ON DELETE CASCADE,
    -- The provider's refresh token, sealed.
    refresh_token TEXT,
    checked_at TEXT NOT NULL
) STRICT;

CREATE INDEX provider_sessions_provider ON provider_sessions (provider_id);
