-- Every audited event once, in one hash chain ordered by sequence.
CREATE TABLE audit_records (
    sequence INTEGER PRIMARY KEY CHECK (sequence > 0),
    event_id TEXT NOT NULL,
    source TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event_version INTEGER NOT NULL,
    subject TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    actor_type TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    causation_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    traceparent TEXT NOT NULL,
    -- Canonical JSON text, kept verbatim so the record hashes the same later.
    data TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    hash TEXT NOT NULL,
    UNIQUE (source, event_id)
) STRICT;

CREATE INDEX audit_records_actor ON audit_records (actor_id, sequence DESC);
CREATE INDEX audit_records_type ON audit_records (event_type, sequence DESC);
CREATE INDEX audit_records_subject ON audit_records (subject, sequence DESC);
CREATE INDEX audit_records_correlation ON audit_records (correlation_id, sequence DESC);
CREATE INDEX audit_records_time ON audit_records (occurred_at, sequence DESC);

-- The last record of the chain; appends rewrite it under the file's write
-- lock, so the chain has one order.
CREATE TABLE audit_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    hash TEXT NOT NULL
) STRICT;

INSERT INTO audit_head (singleton, sequence, hash) VALUES (1, 0, '');

-- The head at regular intervals, to verify the chain against.
CREATE TABLE audit_checkpoints (
    sequence INTEGER PRIMARY KEY CHECK (sequence > 0),
    hash TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE TRIGGER audit_records_no_update BEFORE UPDATE ON audit_records
BEGIN
    SELECT RAISE(ABORT, 'audit records are append-only');
END;

CREATE TRIGGER audit_records_no_delete BEFORE DELETE ON audit_records
BEGIN
    SELECT RAISE(ABORT, 'audit records are append-only');
END;

CREATE TRIGGER audit_checkpoints_no_update BEFORE UPDATE ON audit_checkpoints
BEGIN
    SELECT RAISE(ABORT, 'audit records are append-only');
END;

CREATE TRIGGER audit_checkpoints_no_delete BEFORE DELETE ON audit_checkpoints
BEGIN
    SELECT RAISE(ABORT, 'audit records are append-only');
END;
