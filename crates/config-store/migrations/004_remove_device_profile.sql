-- 004_remove_device_profile.sql: 删除 devices.profile（DeviceProfile 整条链删除）。
-- 由 Rust 迁移逻辑在确认列存在后执行（新库建表已不含该列，直接 DROP 会报错）；
-- 执行后所有 v4 库的 devices 表严格为 (id, name)。
ALTER TABLE devices DROP COLUMN profile;
