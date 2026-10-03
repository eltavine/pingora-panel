-- Accounts that belong to programs: they never have a password and are
-- issued API tokens by account managers.
ALTER TABLE accounts ADD COLUMN service boolean NOT NULL DEFAULT false;

ALTER TABLE accounts ADD CONSTRAINT accounts_service_without_password
    CHECK (NOT service OR (password_hash IS NULL AND NOT break_glass));
