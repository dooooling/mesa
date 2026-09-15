-- 006_device_bootstrap_idempotency：device-bootstrap 幂等键表。
-- 同一 idempotency_key 只允许对应一种请求指纹；重放同 key 同指纹直接返回上次结果，
-- 同 key 不同指纹拒绝（409 由 API 层映射），避免双击/重试产生两台 Device。
CREATE TABLE IF NOT EXISTS bootstrap_idempotency(
    key TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    device_id TEXT NOT NULL,
    endpoint_id TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at_ns INTEGER NOT NULL
);
