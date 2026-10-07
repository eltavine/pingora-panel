-- Channels may also notify through a plugin's notification port (ADR
-- 0044). The plugin delivers, so such a channel seals no URL or signing
-- secret: its target names the plugin and the channel it delivers to.
-- SQLite changes a constraint by rebuilding the table, with foreign key
-- enforcement off so that dropping the old table cascades nothing.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE alert_channels_next (
    channel_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('webhook', 'plugin')),
    target TEXT NOT NULL,
    sealed TEXT CHECK ((kind = 'webhook') = (sealed IS NOT NULL)),
    version INTEGER NOT NULL CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

INSERT INTO alert_channels_next (channel_id, kind, target, sealed, version, created_at, updated_at)
    SELECT channel_id, kind, target, sealed, version, created_at, updated_at FROM alert_channels;

DROP TABLE alert_channels;
ALTER TABLE alert_channels_next RENAME TO alert_channels;

COMMIT;
PRAGMA foreign_keys = ON;
