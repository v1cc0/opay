CREATE TABLE IF NOT EXISTS system_configs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    group_name TEXT NOT NULL DEFAULT 'general',
    label TEXT,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
)
