-- Durable jobs. A running job holds a lease owned by one worker attempt;
-- writes with a lease are fenced by owner and attempt.
CREATE TABLE jobs (
    job_id uuid PRIMARY KEY,
    kind text NOT NULL,
    idempotency_key text NOT NULL,
    state text NOT NULL
        CHECK (state IN ('queued', 'running', 'retrying', 'succeeded', 'failed', 'cancelled')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts integer NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    media_type text NOT NULL,
    payload bytea NOT NULL,
    priority smallint NOT NULL DEFAULT 0,
    run_after timestamptz NOT NULL,
    maintenance_window text,
    cancel_requested boolean NOT NULL DEFAULT false,
    lease_owner text,
    lease_expires_at timestamptz,
    progress_percent smallint CHECK (progress_percent BETWEEN 0 AND 100),
    progress_message text,
    last_error_code text,
    last_error_message text,
    correlation_id text NOT NULL,
    causation_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    UNIQUE (kind, idempotency_key),
    CHECK ((state = 'running') = (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL))
);

CREATE INDEX jobs_claimable ON jobs (priority DESC, run_after)
    WHERE state IN ('queued', 'retrying');
CREATE INDEX jobs_leased ON jobs (lease_expires_at) WHERE state = 'running';

-- Recurring jobs: one job per occurrence of an RFC 5545 recurrence.
CREATE TABLE schedules (
    name text PRIMARY KEY,
    recurrence text NOT NULL,
    kind text NOT NULL,
    media_type text NOT NULL,
    payload bytea NOT NULL,
    max_attempts integer NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    priority smallint NOT NULL,
    maintenance_window text,
    enabled boolean NOT NULL,
    next_run_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX schedules_due ON schedules (next_run_at) WHERE enabled;

-- Periods during which jobs that require them may start.
CREATE TABLE maintenance_windows (
    name text PRIMARY KEY,
    recurrence text NOT NULL,
    duration_seconds integer NOT NULL CHECK (duration_seconds > 0),
    updated_at timestamptz NOT NULL DEFAULT now()
);
