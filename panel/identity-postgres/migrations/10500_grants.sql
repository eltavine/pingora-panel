-- Roles given with a scope and conditions; a role a grant gives cannot be
-- deleted.
CREATE TABLE grants (
    id uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    role_id text NOT NULL REFERENCES roles (id),
    scope jsonb NOT NULL,
    conditions jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    created_by text NOT NULL
);

CREATE INDEX grants_account ON grants (account_id);
