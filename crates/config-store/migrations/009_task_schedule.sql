-- 009_task_schedule.sql: Foundation-2 调度单真值（ADR 0003 §37.3）。
-- tasks / event_tasks 表：mode + interval_ms 两列由 schedule_json 单列替代。
-- 新库建表已为新形态（含 schedule_json）；旧库走迁移：
--   1. tasks/event_tasks 加 schedule_json 列（可空过渡）；
--   2. 逐行回填：mode=poll → {"mode":"poll","interval_ms":<interval_ms>}；
--      mode=subscribe → {"mode":"subscribe"}（缺省订阅参数，见 TaskSchedule 反序列化默认值）；
--      interval_ms 缺失的 poll 视为 0（启动/读取时由 validate 拒绝，不在此静默修）；
--   3. 回填后 schedule_json 设 NOT NULL（SQLite 不支持列级 ALTER，采用重建表方式见 Rust 侧）。
-- NOTE：本 SQL 只做加列；回填与重建由 Rust 迁移逻辑执行（需读旧列逐行写新列），
-- 保证 tasks 表的 legacy mode/interval 语义在此版本有意识升级（ADR 0003 决策 1：
-- legacy binding 配置格式不保留兼容，旧库任务在新版本下按新契约解读）。
ALTER TABLE tasks ADD COLUMN schedule_json TEXT NULL;
ALTER TABLE event_tasks ADD COLUMN schedule_json TEXT NULL;
