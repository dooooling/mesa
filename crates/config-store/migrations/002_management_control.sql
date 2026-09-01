-- 002_management_control.sql: §6.5 endpoint_secrets + §6.6 control_audit + schema_migrations
CREATE TABLE IF NOT EXISTS schema_migrations(
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at_ns INTEGER NOT NULL
);

-- §6.5 endpoint_secrets（仅存 ciphertext，非明文）
CREATE TABLE IF NOT EXISTS endpoint_secrets(
    endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    field_path TEXT NOT NULL,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    algorithm TEXT NOT NULL,
    key_id TEXT NOT NULL,
    updated_at_ns INTEGER NOT NULL,
    PRIMARY KEY(endpoint_id, field_path)
);
CREATE INDEX IF NOT EXISTS idx_endpoint_secrets_endpoint
    ON endpoint_secrets(endpoint_id);

-- §6.6 control_audit（审计不随 Endpoint 删除而级联）
CREATE TABLE IF NOT EXISTS control_audit(
    request_id TEXT PRIMARY KEY,
    endpoint_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    operation_type TEXT NOT NULL CHECK(operation_type IN ('write','command')),
    operation_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    result_json TEXT,
    status TEXT NOT NULL,
    started_at_ns INTEGER NOT NULL,
    finished_at_ns INTEGER
);
CREATE INDEX IF NOT EXISTS idx_control_audit_endpoint_time
    ON control_audit(endpoint_id, started_at_ns DESC);
CREATE INDEX IF NOT EXISTS idx_control_audit_status_time
    ON control_audit(status, started_at_ns DESC);
