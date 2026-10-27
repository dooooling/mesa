-- 010_control_audit_lifecycle.sql: control_audit 允许 lifecycle 操作类型（Issue #102）
--
-- 为什么要重建整张表：002 里的 CHECK(operation_type IN ('write','command')) 是表级约束，
-- SQLite 不支持 ALTER TABLE ... DROP/ALTER CONSTRAINT，只能走官方推荐的
-- 「重命名旧表 → 建新表 → 全量复制 → 删旧表 → 重建索引」流程。
--
-- 两个必须守住的不变量：
-- 1. 历史审计行**逐列复制**，不重算、不裁剪、不丢弃（审计的可用性依赖历史完整性）；
-- 2. 索引名保持与 v9 完全一致，查询计划与既有 REST 分页行为不因迁移改变。
--
-- 注意：索引必须先 DROP 再重建。SQLite 的 ALTER TABLE RENAME 会把索引留在被重命名的
-- 表上（索引名不变），若不先删除，新表上创建同名索引必然冲突。
DROP INDEX IF EXISTS idx_control_audit_endpoint_time;
DROP INDEX IF EXISTS idx_control_audit_status_time;

ALTER TABLE control_audit RENAME TO control_audit_legacy_v9;

CREATE TABLE control_audit(
    request_id TEXT PRIMARY KEY,
    endpoint_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    operation_type TEXT NOT NULL CHECK(operation_type IN ('write','command','lifecycle')),
    operation_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    result_json TEXT,
    status TEXT NOT NULL,
    started_at_ns INTEGER NOT NULL,
    finished_at_ns INTEGER
);

INSERT INTO control_audit(
    request_id,endpoint_id,actor,operation_type,operation_id,
    request_json,result_json,status,started_at_ns,finished_at_ns
)
SELECT
    request_id,endpoint_id,actor,operation_type,operation_id,
    request_json,result_json,status,started_at_ns,finished_at_ns
FROM control_audit_legacy_v9;

DROP TABLE control_audit_legacy_v9;

CREATE INDEX idx_control_audit_endpoint_time
    ON control_audit(endpoint_id, started_at_ns DESC);
CREATE INDEX idx_control_audit_status_time
    ON control_audit(status, started_at_ns DESC);
