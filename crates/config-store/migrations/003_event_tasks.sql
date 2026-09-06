-- 003_event_tasks.sql: PR7 EventTask 持久化（v1.1 §10）
-- 事件任务配置快照（与 tasks 表对等，全量 replace + revision）。
-- NOTE: endpoints 删除时配置行级联清理（与 tasks 表一致）；
-- 这只影响"配置"，events.db 中的事件历史不受任何影响（无 FK，不联动）。
CREATE TABLE IF NOT EXISTS event_tasks(
    endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    mode TEXT NOT NULL,
    interval_ms INTEGER,
    binding_kind TEXT NOT NULL,
    binding_config_json TEXT NOT NULL,
    PRIMARY KEY(endpoint_id, id)
);
