-- Transactional outbox. Producers insert in the transaction that changes
-- their state; the relay publishes committed rows in position order.
CREATE TABLE outbox (
    position BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    event_type TEXT NOT NULL,
    subject TEXT NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The event in the CloudEvents Protobuf format.
    cloudevent BYTEA NOT NULL CHECK (octet_length(cloudevent) <= 65536),
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_attempt_at TIMESTAMPTZ,
    published_at TIMESTAMPTZ
);

CREATE INDEX outbox_pending ON outbox (position) WHERE published_at IS NULL;
CREATE INDEX outbox_published ON outbox (published_at) WHERE published_at IS NOT NULL;

-- NOTIFY is delivered only after the inserting transaction commits, so a
-- wakeup always refers to visible rows.
CREATE FUNCTION outbox_notify() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_catalog.pg_notify(TG_TABLE_SCHEMA || '_outbox', '');
    RETURN NULL;
END
$$;

CREATE TRIGGER outbox_notify
AFTER INSERT ON outbox
FOR EACH STATEMENT
EXECUTE FUNCTION outbox_notify();
