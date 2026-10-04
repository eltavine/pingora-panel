-- Alert rules, the channels they notify, where each rule stands and the
-- notifications sent (ADR 0027).

-- Where a channel sends is sealed with its signing secret, since a webhook
-- URL can authorize whoever holds it; `target` keeps only its origin.
CREATE TABLE alert_channels (
    channel_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('webhook')),
    target TEXT NOT NULL,
    sealed TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE alert_rules (
    rule_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    measure TEXT NOT NULL CHECK (measure IN (
        'server_error_ratio', 'latency_p95', 'request_rate', 'upstream_error_ratio',
        'open_connections'
    )),
    comparison TEXT NOT NULL CHECK (comparison IN ('above', 'below')),
    threshold REAL NOT NULL,
    pending_seconds INTEGER NOT NULL CHECK (pending_seconds BETWEEN 0 AND 86400),
    site_id TEXT,
    route_id TEXT CHECK (route_id IS NULL OR site_id IS NOT NULL),
    upstream_id TEXT,
    severity TEXT NOT NULL CHECK (severity IN ('warning', 'critical')),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- A channel that rules name cannot be deleted.
CREATE TABLE alert_rule_channels (
    rule_id TEXT NOT NULL REFERENCES alert_rules (rule_id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL REFERENCES alert_channels (channel_id),
    PRIMARY KEY (rule_id, channel_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX alert_rule_channels_channel ON alert_rule_channels (channel_id);

-- A rule without a row has not been evaluated yet.
CREATE TABLE alert_states (
    rule_id TEXT PRIMARY KEY REFERENCES alert_rules (rule_id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('inactive', 'pending', 'firing')),
    -- When the condition began to hold; null while inactive.
    active_since TEXT CHECK ((state = 'inactive') = (active_since IS NULL)),
    fired_at TEXT CHECK ((state = 'firing') = (fired_at IS NOT NULL)),
    value REAL,
    evaluated_at TEXT NOT NULL,
    evaluation_error TEXT NOT NULL DEFAULT ''
) STRICT;

-- Notifications outlive their rule, so the history stays readable.
CREATE TABLE alert_notifications (
    notification_id BLOB PRIMARY KEY CHECK (length(notification_id) = 16),
    rule_id TEXT NOT NULL,
    channel_id TEXT NOT NULL REFERENCES alert_channels (channel_id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('firing', 'resolved')),
    -- The Alertmanager webhook body, as JSON text.
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    state TEXT NOT NULL CHECK (state IN ('queued', 'delivered', 'abandoned')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at TEXT NOT NULL,
    next_attempt_at TEXT CHECK ((state = 'queued') = (next_attempt_at IS NOT NULL)),
    delivered_at TEXT,
    last_failure TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX alert_notifications_due ON alert_notifications (next_attempt_at)
    WHERE state = 'queued';
CREATE INDEX alert_notifications_recent ON alert_notifications (created_at DESC);
CREATE INDEX alert_notifications_channel ON alert_notifications (channel_id);
