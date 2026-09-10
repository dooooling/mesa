-- 005: endpoints 增加展示名 name（PR25 Endpoint domain 收口）。
-- 旧行默认空字符串；非空约束由 API/Store 层对新写入强制执行。
ALTER TABLE endpoints ADD COLUMN name TEXT NOT NULL DEFAULT '';
