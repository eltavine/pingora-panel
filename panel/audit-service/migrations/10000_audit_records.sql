-- Every audited event once, in one hash chain ordered by sequence.
CREATE TABLE audit_records (
    sequence bigint PRIMARY KEY CHECK (sequence > 0),
    event_id text NOT NULL,
    source text NOT NULL,
    event_type text NOT NULL,
    event_version integer NOT NULL,
    subject text NOT NULL,
    occurred_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL,
    actor_type text NOT NULL,
    actor_id text NOT NULL,
    correlation_id text NOT NULL,
    causation_id text NOT NULL,
    idempotency_key text NOT NULL,
    traceparent text NOT NULL,
    -- Canonical JSON text, kept verbatim so the record hashes the same later.
    data text NOT NULL,
    previous_hash text NOT NULL,
    hash text NOT NULL,
    UNIQUE (source, event_id)
);

CREATE INDEX audit_records_actor ON audit_records (actor_id, sequence DESC);
CREATE INDEX audit_records_type ON audit_records (event_type text_pattern_ops, sequence DESC);
CREATE INDEX audit_records_subject ON audit_records (subject, sequence DESC);
CREATE INDEX audit_records_correlation ON audit_records (correlation_id, sequence DESC);
CREATE INDEX audit_records_time ON audit_records (occurred_at, sequence DESC);

-- The last record of the chain; each append holds its row lock.
CREATE TABLE audit_head (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    sequence bigint NOT NULL CHECK (sequence >= 0),
    hash text NOT NULL
);

INSERT INTO audit_head (singleton, sequence, hash) VALUES (true, 0, '');

-- The head at regular intervals, to verify the chain against.
CREATE TABLE audit_checkpoints (
    sequence bigint PRIMARY KEY CHECK (sequence > 0),
    hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE FUNCTION audit_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit records are append-only' USING ERRCODE = 'insufficient_privilege';
END;
$$;

CREATE TRIGGER audit_records_append_only
    BEFORE UPDATE OR DELETE ON audit_records
    FOR EACH ROW EXECUTE FUNCTION audit_append_only();

CREATE TRIGGER audit_records_no_truncate
    BEFORE TRUNCATE ON audit_records
    FOR EACH STATEMENT EXECUTE FUNCTION audit_append_only();

CREATE TRIGGER audit_checkpoints_append_only
    BEFORE UPDATE OR DELETE ON audit_checkpoints
    FOR EACH ROW EXECUTE FUNCTION audit_append_only();

CREATE TRIGGER audit_checkpoints_no_truncate
    BEFORE TRUNCATE ON audit_checkpoints
    FOR EACH STATEMENT EXECUTE FUNCTION audit_append_only();
