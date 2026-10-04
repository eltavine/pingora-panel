-- Transactional outbox. Producers insert in the transaction that changes
-- their state; the relay publishes committed rows in position order.
CREATE TABLE outbox (
    position INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id BLOB NOT NULL UNIQUE CHECK (length(event_id) = 16),
    event_type TEXT NOT NULL,
    subject TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    -- The event in the CloudEvents Protobuf format.
    cloudevent BLOB NOT NULL CHECK (length(cloudevent) <= 65536),
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_attempt_at TEXT,
    published_at TEXT
) STRICT;

CREATE INDEX outbox_pending ON outbox (position) WHERE published_at IS NULL;
CREATE INDEX outbox_published ON outbox (published_at) WHERE published_at IS NOT NULL;
