-- Named sets of permissions; the built-in ones are rewritten by the service
-- on start so they follow the release's permission catalog.
CREATE TABLE roles (
    id TEXT PRIMARY KEY CHECK (
        length(id) BETWEEN 1 AND 64
        AND substr(id, 1, 1) GLOB '[a-z0-9]'
        AND id NOT GLOB '*[^a-z0-9._-]*'
    ),
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    -- A JSON array of permission names.
    permissions TEXT NOT NULL CHECK (json_type(permissions) = 'array'),
    built_in INTEGER NOT NULL DEFAULT 0 CHECK (built_in IN (0, 1))
) STRICT;

CREATE TABLE accounts (
    id BLOB PRIMARY KEY CHECK (length(id) = 16),
    username TEXT NOT NULL UNIQUE CHECK (
        length(username) BETWEEN 1 AND 64
        AND substr(username, 1, 1) GLOB '[a-z0-9]'
        AND username NOT GLOB '*[^a-z0-9._-]*'
    ),
    display_name TEXT,
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    -- An Argon2id PHC string; without one the account cannot log in.
    password_hash TEXT,
    password_changed_at TEXT,
    -- Consecutive failed logins, the time before which no attempt is
    -- accepted, and whether failures disabled the password.
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_after TEXT,
    locked INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    -- Accounts that keep password sign-in when it is limited to them.
    break_glass INTEGER NOT NULL DEFAULT 0 CHECK (break_glass IN (0, 1)),
    -- Accounts that belong to programs: they never have a password and are
    -- issued API tokens by account managers.
    service INTEGER NOT NULL DEFAULT 0 CHECK (service IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_login_at TEXT,
    CONSTRAINT accounts_service_without_password
        CHECK (NOT service OR (password_hash IS NULL AND NOT break_glass))
) STRICT;

CREATE TABLE role_bindings (
    account_id BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    role_id TEXT NOT NULL REFERENCES roles (id),
    PRIMARY KEY (account_id, role_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX role_bindings_role ON role_bindings (role_id);

-- Secrets are stored only as their SHA-256 hashes.
CREATE TABLE sessions (
    id BLOB PRIMARY KEY CHECK (length(id) = 16),
    account_id BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    secret_hash BLOB NOT NULL UNIQUE CHECK (length(secret_hash) = 32),
    transport TEXT NOT NULL CHECK (transport IN ('cookie', 'bearer')),
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    client_address TEXT,
    user_agent TEXT,
    revoked_at TEXT,
    revoke_reason TEXT
) STRICT;

CREATE INDEX sessions_account ON sessions (account_id, created_at DESC);
CREATE INDEX sessions_expiry ON sessions (expires_at);

CREATE TABLE api_tokens (
    id BLOB PRIMARY KEY CHECK (length(id) = 16),
    account_id BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    secret_hash BLOB NOT NULL UNIQUE CHECK (length(secret_hash) = 32),
    -- A JSON array of permission names.
    permissions TEXT NOT NULL CHECK (json_type(permissions) = 'array'),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT
) STRICT;

CREATE INDEX api_tokens_account ON api_tokens (account_id, created_at DESC);
