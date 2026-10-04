-- One row per activation idempotency key. A row without a receipt is an
-- activation in progress; the receipt is the Protobuf-encoded
-- pingora.panel.config.v1.ActivationReceipt replayed for retries.
CREATE TABLE activation_receipts (
    idempotency_key TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    receipt BLOB,
    claimed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    completed_at TEXT,
    CHECK ((receipt IS NULL) = (completed_at IS NULL))
) STRICT;
