-- The draft written in the configuration language, file by file, beside the
-- model it describes. Drafts stored before the language existed are printed
-- from their model when first read.
ALTER TABLE draft_configuration ADD COLUMN sources jsonb;

-- Every configuration that was applied or attempted. A revision's files never
-- change; its note and outcome do.
CREATE TABLE configuration_revisions (
    id bigserial PRIMARY KEY,
    draft_version bigint NOT NULL CHECK (draft_version > 0),
    language_version integer NOT NULL,
    sources jsonb NOT NULL,
    content_hash text NOT NULL,
    author text NOT NULL,
    note text,
    created_at timestamptz NOT NULL DEFAULT now(),
    outcome text NOT NULL
        CHECK (outcome IN ('applying', 'active', 'superseded', 'rejected', 'failed')),
    outcome_at timestamptz NOT NULL DEFAULT now(),
    -- Why a rejected or failed attempt did not run.
    diagnostics jsonb,
    -- The runtime snapshot the gateway activated.
    snapshot_hash text,
    gateway_revision bigint
);

CREATE UNIQUE INDEX configuration_revisions_one_active
    ON configuration_revisions ((outcome)) WHERE outcome = 'active';
