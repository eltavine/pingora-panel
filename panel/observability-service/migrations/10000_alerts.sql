-- Alert rules, the channels they notify, where each rule stands and the
-- notifications sent (ADR 0027).

-- Where a channel sends is sealed with its signing secret, since a webhook
-- URL can authorize whoever holds it; `target` keeps only its origin.
CREATE TABLE alert_channels (
    channel_id text PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('webhook')),
    target text NOT NULL,
    sealed text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE TABLE alert_rules (
    rule_id text PRIMARY KEY,
    name text NOT NULL,
    description text NOT NULL,
    measure text NOT NULL CHECK (measure IN (
        'server_error_ratio', 'latency_p95', 'request_rate', 'upstream_error_ratio',
        'open_connections'
    )),
    comparison text NOT NULL CHECK (comparison IN ('above', 'below')),
    threshold double precision NOT NULL,
    pending_seconds integer NOT NULL CHECK (pending_seconds BETWEEN 0 AND 86400),
    site_id text,
    route_id text CHECK (route_id IS NULL OR site_id IS NOT NULL),
    upstream_id text,
    severity text NOT NULL CHECK (severity IN ('warning', 'critical')),
    enabled boolean NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

-- A channel that rules name cannot be deleted.
CREATE TABLE alert_rule_channels (
    rule_id text NOT NULL REFERENCES alert_rules (rule_id) ON DELETE CASCADE,
    channel_id text NOT NULL REFERENCES alert_channels (channel_id),
    PRIMARY KEY (rule_id, channel_id)
);

-- A rule without a row has not been evaluated yet.
CREATE TABLE alert_states (
    rule_id text PRIMARY KEY REFERENCES alert_rules (rule_id) ON DELETE CASCADE,
    state text NOT NULL CHECK (state IN ('inactive', 'pending', 'firing')),
    -- When the condition began to hold; null while inactive.
    active_since timestamptz CHECK ((state = 'inactive') = (active_since IS NULL)),
    fired_at timestamptz CHECK ((state = 'firing') = (fired_at IS NOT NULL)),
    value double precision,
    evaluated_at timestamptz NOT NULL,
    evaluation_error text NOT NULL DEFAULT ''
);

-- Notifications outlive their rule, so the history stays readable.
CREATE TABLE alert_notifications (
    notification_id uuid PRIMARY KEY,
    rule_id text NOT NULL,
    channel_id text NOT NULL REFERENCES alert_channels (channel_id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('firing', 'resolved')),
    payload jsonb NOT NULL,
    state text NOT NULL CHECK (state IN ('queued', 'delivered', 'abandoned')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at timestamptz NOT NULL,
    next_attempt_at timestamptz CHECK ((state = 'queued') = (next_attempt_at IS NOT NULL)),
    delivered_at timestamptz,
    last_failure text NOT NULL DEFAULT ''
);

CREATE INDEX alert_notifications_due ON alert_notifications (next_attempt_at)
    WHERE state = 'queued';
CREATE INDEX alert_notifications_recent ON alert_notifications (created_at DESC);
