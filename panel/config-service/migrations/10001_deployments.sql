-- Documents of prepared deployments, kept so that an interrupted activation
-- can be completed and the desired configuration restored to a gateway that
-- lost its state.
CREATE TABLE prepared_deployments (
    prepare_token text PRIMARY KEY,
    revision_id bigint NOT NULL,
    content_hash text NOT NULL,
    schema_version text NOT NULL,
    media_type text NOT NULL,
    content bytea NOT NULL,
    prepared_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX prepared_deployments_content_hash ON prepared_deployments (content_hash);

-- What each activation was asked to do, recorded before it claims its
-- idempotency key so that an interrupted activation can be re-issued.
CREATE TABLE activation_intents (
    idempotency_key text PRIMARY KEY,
    prepare_token text NOT NULL,
    expected_active_hash text,
    actor text NOT NULL,
    correlation_id text NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT now()
);

-- The configuration the gateway must run: the newest one activated.
CREATE TABLE desired_configuration (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    prepare_token text NOT NULL REFERENCES prepared_deployments (prepare_token),
    revision_id bigint NOT NULL,
    content_hash text NOT NULL,
    activated_at timestamptz NOT NULL DEFAULT now()
);
