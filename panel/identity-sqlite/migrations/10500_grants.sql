-- Roles given with a scope and conditions; a role a grant gives cannot be
-- deleted.
CREATE TABLE grants (
    id BLOB PRIMARY KEY CHECK (length(id) = 16),
    account_id BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    role_id TEXT NOT NULL REFERENCES roles (id),
    scope TEXT NOT NULL CHECK (json_valid(scope)),
    conditions TEXT NOT NULL CHECK (json_valid(conditions)),
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL
) STRICT;

CREATE INDEX grants_account ON grants (account_id);
CREATE INDEX grants_role ON grants (role_id);
