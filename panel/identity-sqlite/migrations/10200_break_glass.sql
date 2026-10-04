-- Who may sign in with a password; one row. Without a rowid, the key is an
-- ordinary column that takes its default.
CREATE TABLE sign_in_policy (
    singleton INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    password_sign_in TEXT NOT NULL CHECK (password_sign_in IN ('everyone', 'break_glass_only')),
    updated_at TEXT NOT NULL
) STRICT, WITHOUT ROWID;

INSERT INTO sign_in_policy (password_sign_in, updated_at)
VALUES ('everyone', strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'));
