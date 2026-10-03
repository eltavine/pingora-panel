-- Named sets of permissions; the built-in ones are rewritten by the service
-- on start so they follow the release's permission catalog.
CREATE TABLE roles (
    id text PRIMARY KEY CHECK (id ~ '^[a-z0-9][a-z0-9._-]{0,63}$'),
    name text NOT NULL,
    description text NOT NULL,
    permissions text[] NOT NULL,
    built_in boolean NOT NULL DEFAULT false
);

CREATE TABLE accounts (
    id uuid PRIMARY KEY,
    username text NOT NULL UNIQUE CHECK (username ~ '^[a-z0-9][a-z0-9._-]{0,63}$'),
    display_name text,
    disabled boolean NOT NULL DEFAULT false,
    -- An Argon2id PHC string; without one the account cannot log in.
    password_hash text,
    password_changed_at timestamptz,
    -- Consecutive failed logins, the time before which no attempt is
    -- accepted, and whether failures disabled the password.
    failures integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_after timestamptz,
    locked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    last_login_at timestamptz
);

CREATE TABLE role_bindings (
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    role_id text NOT NULL REFERENCES roles (id),
    PRIMARY KEY (account_id, role_id)
);

-- Secrets are stored only as their SHA-256 hashes.
CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    secret_hash bytea NOT NULL UNIQUE CHECK (octet_length(secret_hash) = 32),
    csrf_hash bytea NOT NULL CHECK (octet_length(csrf_hash) = 32),
    transport text NOT NULL CHECK (transport IN ('cookie', 'bearer')),
    created_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    client_address text,
    user_agent text,
    revoked_at timestamptz,
    revoke_reason text
);

CREATE INDEX sessions_account ON sessions (account_id, created_at DESC);
CREATE INDEX sessions_expiry ON sessions (expires_at);

CREATE TABLE api_tokens (
    id uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name text NOT NULL,
    secret_hash bytea NOT NULL UNIQUE CHECK (octet_length(secret_hash) = 32),
    permissions text[] NOT NULL,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    last_used_at timestamptz,
    revoked_at timestamptz
);

CREATE INDEX api_tokens_account ON api_tokens (account_id, created_at DESC);
