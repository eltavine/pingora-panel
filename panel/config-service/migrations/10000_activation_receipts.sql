-- One row per activation idempotency key. A row without a receipt is an
-- activation in progress; the receipt is the Protobuf-encoded
-- pingora.panel.config.v1.ActivationReceipt replayed for retries.
CREATE TABLE activation_receipts (
    idempotency_key text PRIMARY KEY,
    request_hash text NOT NULL,
    receipt bytea,
    claimed_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CHECK ((receipt IS NULL) = (completed_at IS NULL))
);
