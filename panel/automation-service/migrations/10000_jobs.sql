-- Durable jobs. A running job holds a lease owned by one worker attempt;
-- writes with a lease are fenced by owner and attempt.
CREATE TABLE jobs (
    job_id BLOB PRIMARY KEY CHECK (length(job_id) = 16),
    kind TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN ('queued', 'running', 'retrying', 'succeeded', 'failed', 'cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    media_type TEXT NOT NULL,
    payload BLOB NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0 CHECK (priority BETWEEN -32768 AND 32767),
    run_after TEXT NOT NULL,
    maintenance_window TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
    lease_owner TEXT,
    lease_expires_at TEXT,
    progress_percent INTEGER CHECK (progress_percent BETWEEN 0 AND 100),
    progress_message TEXT,
    last_error_code TEXT,
    last_error_message TEXT,
    correlation_id TEXT NOT NULL,
    causation_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT,
    UNIQUE (kind, idempotency_key),
    CHECK ((state = 'running') = (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL))
) STRICT;

CREATE INDEX jobs_claimable ON jobs (priority DESC, run_after)
    WHERE state IN ('queued', 'retrying');
CREATE INDEX jobs_leased ON jobs (lease_expires_at) WHERE state = 'running';

-- Recurring jobs: one job per occurrence of an RFC 5545 recurrence.
CREATE TABLE schedules (
    name TEXT PRIMARY KEY,
    recurrence TEXT NOT NULL,
    kind TEXT NOT NULL,
    media_type TEXT NOT NULL,
    payload BLOB NOT NULL,
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    priority INTEGER NOT NULL CHECK (priority BETWEEN -32768 AND 32767),
    maintenance_window TEXT,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    next_run_at TEXT,
    updated_at TEXT NOT NULL
) STRICT;

CREATE INDEX schedules_due ON schedules (next_run_at) WHERE enabled;

-- Periods during which jobs that require them may start.
CREATE TABLE maintenance_windows (
    name TEXT PRIMARY KEY,
    recurrence TEXT NOT NULL,
    duration_seconds INTEGER NOT NULL CHECK (duration_seconds > 0),
    updated_at TEXT NOT NULL
) STRICT;
