-- 007_point_source_label：point_registry 新增 source_label（Point Presentation Metadata P1）。
-- Driver 在 configure 回执里给的人类可读来源（如 S7 的 DB10.DBD20），纯展示元数据，
-- 不是 identity：point_id/point_key 不变，label 随每次 configure 快照更新（含 None 清空）。
ALTER TABLE point_registry ADD COLUMN source_label TEXT NULL;
