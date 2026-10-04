-- Documents of prepared deployments, kept so that an interrupted activation
-- can be completed and the desired configuration restored to a gateway that
-- lost its state.
CREATE TABLE prepared_deployments (
    prepare_token TEXT PRIMARY KEY,
    revision_id INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    media_type TEXT NOT NULL,
    content BLOB NOT NULL,
    prepared_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
) STRICT;

CREATE INDEX prepared_deployments_content_hash ON prepared_deployments (content_hash);

-- What each activation was asked to do, recorded before it claims its
-- idempotency key so that an interrupted activation can be re-issued.
CREATE TABLE activation_intents (
    idempotency_key TEXT PRIMARY KEY,
    prepare_token TEXT NOT NULL,
    expected_active_hash TEXT,
    actor TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
) STRICT;

CREATE INDEX activation_intents_prepare_token ON activation_intents (prepare_token);

-- The configuration the gateway must run: the newest one activated. Without
-- a rowid, the key is an ordinary column that takes its default.
CREATE TABLE desired_configuration (
    singleton INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    prepare_token TEXT NOT NULL REFERENCES prepared_deployments (prepare_token),
    revision_id INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    activated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
) STRICT, WITHOUT ROWID;
