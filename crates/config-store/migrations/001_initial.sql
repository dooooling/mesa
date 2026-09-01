-- 001_initial.sql: 基线 schema（与原 migrate() 内联一致）
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS meta(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS devices(
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    profile TEXT
);
CREATE TABLE IF NOT EXISTS endpoints(
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
    driver_id TEXT NOT NULL,
    connection_json TEXT NOT NULL,
    desired_running INTEGER NOT NULL DEFAULT 0,
    updated_at_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tasks(
    endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    mode TEXT NOT NULL,
    interval_ms INTEGER,
    binding_kind TEXT NOT NULL,
    binding_config_json TEXT NOT NULL,
    PRIMARY KEY(endpoint_id, id)
);
CREATE TABLE IF NOT EXISTS point_registry(
    endpoint_id TEXT NOT NULL,
    point_key TEXT NOT NULL,
    point_id INTEGER NOT NULL,
    data_type TEXT NOT NULL,
    unit TEXT,
    deleted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(endpoint_id, point_key)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_point_registry_ep_pid
    ON point_registry(endpoint_id, point_id);
CREATE TABLE IF NOT EXISTS config_revision(
    endpoint_id TEXT PRIMARY KEY REFERENCES endpoints(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL
);
