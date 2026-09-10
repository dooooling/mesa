-- 005: endpoints 增加展示名 name（PR25 Endpoint domain 收口）。
-- 旧行以 id 回填（id 现成、稳定、非空），保证 v5 不变量：所有 name 非空。
ALTER TABLE endpoints ADD COLUMN name TEXT NOT NULL DEFAULT '';
UPDATE endpoints SET name = id WHERE trim(name) = '';
