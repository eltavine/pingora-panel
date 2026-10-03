-- Accounts that keep password sign-in when it is limited to them.
ALTER TABLE accounts ADD COLUMN break_glass boolean NOT NULL DEFAULT false;

-- Who may sign in with a password; one row.
CREATE TABLE sign_in_policy (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    password_sign_in text NOT NULL CHECK (password_sign_in IN ('everyone', 'break_glass_only')),
    updated_at timestamptz NOT NULL
);

INSERT INTO sign_in_policy (password_sign_in, updated_at) VALUES ('everyone', now());
