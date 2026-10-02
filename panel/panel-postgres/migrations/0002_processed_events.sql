-- Idempotent consumers: the events each consumer has claimed or finished.
-- A row is either an unexpired claim or a completion, never both.
CREATE TABLE processed_events (
    consumer TEXT NOT NULL,
    event_id UUID NOT NULL,
    claimed_until TIMESTAMPTZ,
    processed_at TIMESTAMPTZ,
    PRIMARY KEY (consumer, event_id),
    CHECK ((claimed_until IS NULL) <> (processed_at IS NULL))
);

CREATE INDEX processed_events_retention ON processed_events (processed_at)
WHERE processed_at IS NOT NULL;
