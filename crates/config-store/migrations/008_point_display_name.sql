-- 008_point_display_name：point_registry 新增 display_name（Point Presentation Metadata P2）。
-- 用户可编辑展示名，与 Driver 无关：configure 不得覆盖/清空它（与 source_label
-- 的快照语义正好相反）。NULL = 未设置，UI 回落 point_key；空字符串非法（API 层拒绝）。
ALTER TABLE point_registry ADD COLUMN display_name TEXT NULL;
