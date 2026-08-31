//! ConfigStore：SQLite 持久化（方案 §4.3、§5、§6）。
//!
//! 职责：Device / Endpoint / Task 全量快照 + PointRegistry（带 tombstone）+
//! 每 Endpoint 的 Revision 与启停期望。运行期 LatestValue 仍驻留内存，不落盘。
//!
//! 并发模型：V1 单管理员/单写者，库内用单 `Mutex<Connection>` 串行化；
//! 所有写操作包在事务内，保证"全量替换要么全成功，要么保持旧版"。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

#[allow(unused_imports)]
use mesa_core_types::{
    AcquisitionTask, DataType, PointDefinition, PointDescriptor, ensure_unique_point_keys,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// 当前 schema 版本。增量迁移时递增并在 `meta` 中持久化。
const SCHEMA_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// 记录类型
// ---------------------------------------------------------------------------

/// Device 记录（§5.2）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    /// 关联的 DeviceProfile id，可空（V1 允许先建设备后补 profile）。
    pub profile: Option<String>,
}

/// Endpoint 记录（§5.3）。`connection` 的语义由 Driver 解释，Core 只做 JSON 透传。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EndpointRecord {
    pub id: String,
    pub device_id: String,
    pub driver_id: String,
    /// 已序列化的 connection JSON（对象）。
    pub connection_json: String,
    /// 期望运行态：true = running，false = stopped。
    pub desired_running: bool,
    pub updated_at_ns: i64,
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("duplicate: {0}")]
    Duplicate(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct ConfigStore {
    conn: Mutex<Connection>,
}

impl ConfigStore {
    /// 打开（不存在则创建）并执行迁移。
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        // 父目录不存在时显式创建，避免 rusqlite 报错信息不直观
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                StoreError::Sqlite(rusqlite::Error::InvalidParameterName(e.to_string()))
            })?;
        }
        let conn = Connection::open(path)?;
        let s = Self {
            conn: Mutex::new(conn),
        };
        s.migrate()?;
        Ok(s)
    }

    /// 内存库（单测/临时使用）。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let s = Self {
            conn: Mutex::new(conn),
        };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // 建表：幂等
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meta(
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS devices(
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                profile TEXT
            );
            CREATE TABLE IF NOT EXISTS endpoints(
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
                driver_id TEXT NOT NULL,
                connection_json TEXT NOT NULL,
                desired_running INTEGER NOT NULL DEFAULT 0,
                updated_at_ns INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks(
                endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
                id TEXT NOT NULL,
                mode TEXT NOT NULL,
                interval_ms INTEGER,
                binding_kind TEXT NOT NULL,
                binding_config_json TEXT NOT NULL,
                PRIMARY KEY(endpoint_id, id)
            );
            CREATE TABLE IF NOT EXISTS point_registry(
                endpoint_id TEXT NOT NULL,
                point_key TEXT NOT NULL,
                point_id INTEGER NOT NULL,
                data_type TEXT NOT NULL,
                unit TEXT,
                deleted INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(endpoint_id, point_key)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_point_registry_ep_pid
                ON point_registry(endpoint_id, point_id);
            CREATE TABLE IF NOT EXISTS config_revision(
                endpoint_id TEXT PRIMARY KEY REFERENCES endpoints(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL
            );
            "#,
        )?;
        // 版本标记
        let cur: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if cur.is_none() {
            conn.execute(
                "INSERT INTO meta(key,value) VALUES('schema_version',?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
        }
        Ok(())
    }

    // ---- helpers ----

    fn now_ns() -> i64 {
        mesa_core_types::now_unix_ns()
    }

    // ---- Device ----

    pub fn create_device(&self, rec: &DeviceRecord) -> Result<(), StoreError> {
        Self::validate_id(&rec.id)?;
        if rec.name.trim().is_empty() {
            return Err(StoreError::Validation("device name 不能为空".into()));
        }
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT INTO devices(id,name,profile) VALUES(?1,?2,?3)",
            params![rec.id, rec.name, rec.profile],
        );
        match n {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::Duplicate(format!("device `{}` 已存在", rec.id)))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id,name,profile FROM devices ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(DeviceRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                profile: r.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_device(&self, id: &str) -> Result<Option<DeviceRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,name,profile FROM devices WHERE id=?1",
            params![id],
            |r| {
                Ok(DeviceRecord {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    profile: r.get(2)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_device(&self, rec: &DeviceRecord) -> Result<bool, StoreError> {
        if rec.name.trim().is_empty() {
            return Err(StoreError::Validation("device name 不能为空".into()));
        }
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE devices SET name=?1, profile=?2 WHERE id=?3",
            params![rec.name, rec.profile, rec.id],
        )?;
        Ok(n > 0)
    }

    pub fn delete_device(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        // 若仍有 endpoint 引用则 RESTRICT，转换为可读错误
        let n = conn.execute("DELETE FROM devices WHERE id=?1", params![id]);
        match n {
            Ok(c) => Ok(c > 0),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::Conflict(format!(
                    "device `{id}` 仍被 endpoint 引用，请先删除关联 endpoint"
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    // ---- Endpoint ----

    pub fn create_endpoint(&self, rec: &EndpointRecord) -> Result<(), StoreError> {
        Self::validate_id(&rec.id)?;
        Self::validate_id(&rec.device_id)?;
        Self::validate_id(&rec.driver_id)?;
        // 校验 connection_json 为合法 JSON 对象
        let v: serde_json::Value =
            serde_json::from_str(&rec.connection_json).map_err(StoreError::Json)?;
        if !v.is_object() {
            return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
        }
        let conn = self.conn.lock().unwrap();
        // 确认 device 存在
        let dev_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1)",
            params![rec.device_id],
            |r| r.get(0),
        )?;
        if !dev_exists {
            return Err(StoreError::NotFound(format!(
                "device `{}` 不存在",
                rec.device_id
            )));
        }
        let n = conn.execute(
            "INSERT INTO endpoints(id,device_id,driver_id,connection_json,desired_running,updated_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![rec.id, rec.device_id, rec.driver_id, rec.connection_json, rec.desired_running as i32, rec.updated_at_ns],
        );
        match n {
            Ok(_) => {
                conn.execute(
                    "INSERT OR IGNORE INTO config_revision(endpoint_id,revision) VALUES(?1,0)",
                    params![rec.id],
                )?;
                Ok(())
            }
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::Duplicate(format!(
                    "endpoint `{}` 已存在",
                    rec.id
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_endpoints(&self) -> Result<Vec<EndpointRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,device_id,driver_id,connection_json,desired_running,updated_at_ns FROM endpoints ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(EndpointRecord {
                id: r.get(0)?,
                device_id: r.get(1)?,
                driver_id: r.get(2)?,
                connection_json: r.get(3)?,
                desired_running: r.get::<_, i32>(4)? != 0,
                updated_at_ns: r.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_endpoint(&self, id: &str) -> Result<Option<EndpointRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,device_id,driver_id,connection_json,desired_running,updated_at_ns FROM endpoints WHERE id=?1",
            params![id],
            |r| {
                Ok(EndpointRecord {
                    id: r.get(0)?,
                    device_id: r.get(1)?,
                    driver_id: r.get(2)?,
                    connection_json: r.get(3)?,
                    desired_running: r.get::<_, i32>(4)? != 0,
                    updated_at_ns: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_endpoint(&self, rec: &EndpointRecord) -> Result<bool, StoreError> {
        let v: serde_json::Value =
            serde_json::from_str(&rec.connection_json).map_err(StoreError::Json)?;
        if !v.is_object() {
            return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
        }
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE endpoints SET device_id=?1, driver_id=?2, connection_json=?3, desired_running=?4, updated_at_ns=?5 WHERE id=?6",
            params![rec.device_id, rec.driver_id, rec.connection_json, rec.desired_running as i32, rec.updated_at_ns, rec.id],
        )?;
        Ok(n > 0)
    }

    pub fn delete_endpoint(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM endpoints WHERE id=?1", params![id])?;
        Ok(n > 0)
    }

    pub fn set_desired_running(
        &self,
        endpoint_id: &str,
        running: bool,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE endpoints SET desired_running=?1, updated_at_ns=?2 WHERE id=?3",
            params![running as i32, Self::now_ns(), endpoint_id],
        )?;
        Ok(n > 0)
    }

    // ---- Tasks（全量快照替换，§6.2）----

    /// 全量替换某 endpoint 的任务集合。空数组表示清空。
    /// 成功时 revision 自增并返回新 revision；失败则事务回滚、旧配置保持不变。
    pub fn replace_tasks(
        &self,
        endpoint_id: &str,
        tasks: &[AcquisitionTask],
    ) -> Result<u64, StoreError> {
        // 结构级校验
        for t in tasks {
            t.validate()
                .map_err(|e| StoreError::Validation(e.to_string()))?;
        }
        // 同 endpoint 内 task id 唯一
        {
            let mut seen = HashSet::new();
            for t in tasks {
                if !seen.insert(&t.id) {
                    return Err(StoreError::Validation(format!(
                        "duplicate task id `{}`",
                        t.id
                    )));
                }
            }
        }

        let mut conn = self.conn.lock().unwrap();
        // 确认 endpoint 存在
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM endpoints WHERE id=?1)",
            params![endpoint_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound(format!(
                "endpoint `{endpoint_id}` 不存在"
            )));
        }

        let tx = conn.transaction()?;
        Self::replace_tasks_in_tx(&tx, endpoint_id, tasks)?;
        // bump revision
        let cur: i64 = tx
            .query_row(
                "SELECT revision FROM config_revision WHERE endpoint_id=?1",
                params![endpoint_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let next = (cur + 1) as u64;
        tx.execute(
            "INSERT INTO config_revision(endpoint_id,revision) VALUES(?1,?2)
             ON CONFLICT(endpoint_id) DO UPDATE SET revision=excluded.revision",
            params![endpoint_id, next as i64],
        )?;
        tx.execute(
            "UPDATE endpoints SET updated_at_ns=?1 WHERE id=?2",
            params![Self::now_ns(), endpoint_id],
        )?;
        tx.commit()?;
        Ok(next)
    }

    fn replace_tasks_in_tx(
        tx: &Transaction<'_>,
        endpoint_id: &str,
        tasks: &[AcquisitionTask],
    ) -> Result<(), StoreError> {
        tx.execute(
            "DELETE FROM tasks WHERE endpoint_id=?1",
            params![endpoint_id],
        )?;
        for t in tasks {
            tx.execute(
                "INSERT INTO tasks(endpoint_id,id,mode,interval_ms,binding_kind,binding_config_json)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    endpoint_id,
                    t.id,
                    t.mode.as_str(),
                    t.interval_ms.map(|v| v as i64),
                    t.binding.kind,
                    serde_json::to_string(&t.binding.config).unwrap(),
                ],
            )?;
        }
        Ok(())
    }

    pub fn list_tasks(&self, endpoint_id: &str) -> Result<Vec<AcquisitionTask>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,mode,interval_ms,binding_kind,binding_config_json FROM tasks WHERE endpoint_id=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![endpoint_id], |r| {
            let mode_s: String = r.get(1)?;
            let mode = match mode_s.as_str() {
                "poll" => mesa_core_types::TaskMode::Poll,
                "subscribe" => mesa_core_types::TaskMode::Subscribe,
                _ => mesa_core_types::TaskMode::Poll,
            };
            let binding_config_json: String = r.get(4)?;
            let cfg: serde_json::Value =
                serde_json::from_str(&binding_config_json).unwrap_or(serde_json::json!({}));
            Ok(AcquisitionTask {
                id: r.get(0)?,
                mode,
                interval_ms: r.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                binding: mesa_core_types::DriverBinding {
                    kind: r.get(3)?,
                    config: cfg,
                },
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn current_revision(&self, endpoint_id: &str) -> Result<u64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let v: Option<i64> = conn
            .query_row(
                "SELECT revision FROM config_revision WHERE endpoint_id=?1",
                params![endpoint_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.unwrap_or(0) as u64)
    }

    // ---- PointRegistry（§6.1 稳定 ID + tombstone）----

    /// 为一批 descriptor 分配稳定 point_id。已存在（含 tombstone）的 key 复用原 id；
    /// 新 key 取 `max(point_id)+1`。同时更新 `data_type/unit` 并清除 `deleted` 标记。
    /// 调用前已由外层保证 `ensure_unique_point_keys`，此处再做一次防御性检查。
    pub fn assign_point_ids(
        &self,
        endpoint_id: &str,
        descriptors: &[PointDescriptor],
    ) -> Result<Vec<PointDefinition>, StoreError> {
        ensure_unique_point_keys(descriptors).map_err(|e| StoreError::Validation(e.to_string()))?;

        let mut conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM endpoints WHERE id=?1)",
            params![endpoint_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound(format!(
                "endpoint `{endpoint_id}` 不存在"
            )));
        }

        let tx = conn.transaction()?;

        // 已有映射
        let mut existing: HashMap<String, (u32, bool)> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT point_key, point_id, deleted FROM point_registry WHERE endpoint_id=?1",
            )?;
            let rows = stmt.query_map(params![endpoint_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as u32,
                    r.get::<_, i32>(2)? != 0,
                ))
            })?;
            for r in rows {
                let (k, id, del) = r?;
                existing.insert(k, (id, del));
            }
        }
        let max_id = existing.values().map(|(id, _)| *id).max().unwrap_or(0);
        let mut next_id = max_id + 1;

        // 标记本轮出现的 key，用于后续 tombstone 处理（删除的 key 保持墓碑，不复用 id）
        let incoming_keys: HashSet<&str> =
            descriptors.iter().map(|d| d.point_key.as_str()).collect();

        let mut out = Vec::with_capacity(descriptors.len());
        for d in descriptors {
            let pid = if let Some((id, _del)) = existing.get(d.point_key.as_str()) {
                *id
            } else {
                let id = next_id;
                next_id += 1;
                id
            };
            // upsert：复用或新增均写入最新类型/unit 并清除 deleted
            tx.execute(
                "INSERT INTO point_registry(endpoint_id,point_key,point_id,data_type,unit,deleted)
                 VALUES(?1,?2,?3,?4,?5,0)
                 ON CONFLICT(endpoint_id,point_key) DO UPDATE SET
                    point_id=excluded.point_id, data_type=excluded.data_type,
                    unit=excluded.unit, deleted=0",
                params![
                    endpoint_id,
                    d.point_key,
                    pid as i64,
                    d.data_type.as_str(),
                    d.unit
                ],
            )?;
            out.push(PointDefinition {
                point_id: pid,
                point_key: d.point_key.clone(),
                data_type: d.data_type,
                unit: d.unit.clone(),
            });
        }

        // 不在 incoming_keys 中的旧活跃点，置墓碑（保留 id，不删除行）
        {
            let mut stmt = tx.prepare(
                "SELECT point_key FROM point_registry WHERE endpoint_id=?1 AND deleted=0",
            )?;
            let active_keys: Vec<String> = stmt
                .query_map(params![endpoint_id], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for k in active_keys {
                if !incoming_keys.contains(k.as_str()) {
                    tx.execute(
                        "UPDATE point_registry SET deleted=1 WHERE endpoint_id=?1 AND point_key=?2",
                        params![endpoint_id, k],
                    )?;
                }
            }
        }

        tx.commit()?;
        Ok(out)
    }

    /// 活跃映射（deleted=0）。
    pub fn point_map(&self, endpoint_id: &str) -> Result<HashMap<String, u32>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT point_key, point_id FROM point_registry WHERE endpoint_id=?1 AND deleted=0",
        )?;
        let rows = stmt.query_map(params![endpoint_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        let mut m = HashMap::new();
        for r in rows {
            let (k, id) = r?;
            m.insert(k, id);
        }
        Ok(m)
    }

    /// 全部（含墓碑），供诊断。
    pub fn point_registry_all(
        &self,
        endpoint_id: &str,
    ) -> Result<Vec<(String, u32, String, bool)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT point_key, point_id, data_type, deleted FROM point_registry WHERE endpoint_id=?1 ORDER BY point_id",
        )?;
        let rows = stmt.query_map(params![endpoint_id], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, i64>(1)? as u32,
                r.get::<_, String>(2)?,
                r.get::<_, i32>(3)? != 0,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ---- 校验 ----

    fn validate_id(id: &str) -> Result<(), StoreError> {
        if id.trim().is_empty() {
            return Err(StoreError::Validation("id 不能为空".into()));
        }
        if id.len() > 128 {
            return Err(StoreError::Validation("id 过长（≤128）".into()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 便捷：把 connection_json 字符串校验为对象
// ---------------------------------------------------------------------------

pub fn validate_connection_json(s: &str) -> Result<serde_json::Value, StoreError> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(StoreError::Json)?;
    if !v.is_object() {
        return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{DriverBinding, TaskMode};

    fn mem() -> ConfigStore {
        ConfigStore::open_in_memory().unwrap()
    }

    fn dev(id: &str) -> DeviceRecord {
        DeviceRecord {
            id: id.into(),
            name: format!("{id}-name"),
            profile: None,
        }
    }

    fn ep(id: &str, device: &str) -> EndpointRecord {
        EndpointRecord {
            id: id.into(),
            device_id: device.into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        }
    }

    fn task(id: &str, interval: u64) -> AcquisitionTask {
        AcquisitionTask {
            id: id.into(),
            mode: TaskMode::Poll,
            interval_ms: Some(interval),
            binding: DriverBinding {
                kind: "simulator.points".into(),
                config: serde_json::json!({}),
            },
        }
    }

    fn desc(key: &str, ty: DataType) -> PointDescriptor {
        PointDescriptor {
            point_key: key.into(),
            data_type: ty,
            unit: None,
        }
    }

    #[test]
    fn device_crud_roundtrip() {
        let s = mem();
        assert!(s.get_device("d1").unwrap().is_none());
        s.create_device(&dev("d1")).unwrap();
        assert!(matches!(
            s.create_device(&dev("d1")),
            Err(StoreError::Duplicate(_))
        ));
        let list = s.list_devices().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "d1");
        // update
        let mut d = dev("d1");
        d.name = "new".into();
        assert!(s.update_device(&d).unwrap());
        assert_eq!(s.get_device("d1").unwrap().unwrap().name, "new");
        // delete
        assert!(s.delete_device("d1").unwrap());
        assert!(!s.delete_device("d1").unwrap());
    }

    #[test]
    fn device_delete_restricted_when_endpoint_exists() {
        let s = mem();
        s.create_device(&dev("plc001")).unwrap();
        s.create_endpoint(&ep("ep1", "plc001")).unwrap();
        assert!(matches!(
            s.delete_device("plc001"),
            Err(StoreError::Conflict(_))
        ));
        s.delete_endpoint("ep1").unwrap();
        assert!(s.delete_device("plc001").unwrap());
    }

    #[test]
    fn endpoint_crud_and_desired_state() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        assert!(matches!(
            s.create_endpoint(&ep("e1", "d1")),
            Err(StoreError::Duplicate(_))
        ));
        // 非法 connection
        let mut bad = ep("e2", "d1");
        bad.connection_json = "not json".into();
        assert!(s.create_endpoint(&bad).is_err());
        // desired_running 切换
        s.set_desired_running("e1", true).unwrap();
        assert!(s.get_endpoint("e1").unwrap().unwrap().desired_running);
        assert_eq!(s.list_endpoints().unwrap().len(), 1);
        assert!(s.delete_endpoint("e1").unwrap());
    }

    #[test]
    fn replace_tasks_full_snapshot_and_revision() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        assert_eq!(s.current_revision("e1").unwrap(), 0);
        let r1 = s.replace_tasks("e1", &[task("t1", 100)]).unwrap();
        assert_eq!(r1, 1);
        assert_eq!(s.current_revision("e1").unwrap(), 1);
        assert_eq!(s.list_tasks("e1").unwrap().len(), 1);
        // 全量替换：旧任务消失
        let r2 = s
            .replace_tasks("e1", &[task("t2", 200), task("t3", 300)])
            .unwrap();
        assert_eq!(r2, 2);
        let tasks = s.list_tasks("e1").unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.id == "t2"));
        assert!(!tasks.iter().any(|t| t.id == "t1"));
        // 清空
        s.replace_tasks("e1", &[]).unwrap();
        assert!(s.list_tasks("e1").unwrap().is_empty());
        assert_eq!(s.current_revision("e1").unwrap(), 3);
    }

    #[test]
    fn replace_tasks_rejects_invalid_and_duplicate_ids() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        let bad = AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: None,
            binding: DriverBinding {
                kind: "k".into(),
                config: serde_json::json!({}),
            },
        };
        assert!(matches!(
            s.replace_tasks("e1", &[bad]),
            Err(StoreError::Validation(_))
        ));
        // 事务回滚：失败后 revision 不变
        assert_eq!(s.current_revision("e1").unwrap(), 0);
        // 重复 id
        assert!(matches!(
            s.replace_tasks("e1", &[task("dup", 100), task("dup", 200)]),
            Err(StoreError::Validation(_))
        ));
    }

    #[test]
    fn point_id_stable_and_tombstone_reuse() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        // 首轮分配
        let defs1 = s
            .assign_point_ids("e1", &[desc("a", DataType::F64), desc("b", DataType::Bool)])
            .unwrap();
        let id_a1 = defs1.iter().find(|d| d.point_key == "a").unwrap().point_id;
        let id_b1 = defs1.iter().find(|d| d.point_key == "b").unwrap().point_id;
        assert_ne!(id_a1, id_b1);
        // 增量加入 c，a/b 复用
        let defs2 = s
            .assign_point_ids(
                "e1",
                &[
                    desc("a", DataType::F64),
                    desc("b", DataType::Bool),
                    desc("c", DataType::I32),
                ],
            )
            .unwrap();
        assert_eq!(
            defs2.iter().find(|d| d.point_key == "a").unwrap().point_id,
            id_a1
        );
        assert_eq!(
            defs2.iter().find(|d| d.point_key == "b").unwrap().point_id,
            id_b1
        );
        let id_c = defs2.iter().find(|d| d.point_key == "c").unwrap().point_id;
        // 删除 b（墓碑），再缩容到仅 a+c
        s.assign_point_ids("e1", &[desc("a", DataType::F64), desc("c", DataType::I32)])
            .unwrap();
        let all = s.point_registry_all("e1").unwrap();
        let b_entry = all.iter().find(|(k, _, _, _)| k == "b").unwrap();
        assert!(b_entry.3, "b 应为墓碑");
        // 重新加入 b，必须复用原 id，且不复用已删 id 给新 key
        let defs3 = s
            .assign_point_ids(
                "e1",
                &[
                    desc("a", DataType::F64),
                    desc("b", DataType::Bool),
                    desc("c", DataType::I32),
                    desc("d", DataType::String),
                ],
            )
            .unwrap();
        assert_eq!(
            defs3.iter().find(|d| d.point_key == "b").unwrap().point_id,
            id_b1
        );
        let id_d = defs3.iter().find(|d| d.point_key == "d").unwrap().point_id;
        assert!(id_d > id_c && id_d != id_b1, "新 key 不得复用墓碑 id");
        // 活跃映射：本轮 a,b,c,d 均已恢复/存在
        let map = s.point_map("e1").unwrap();
        assert_eq!(map.len(), 4);
    }

    #[test]
    fn point_id_rejects_duplicate_key() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        let err = s
            .assign_point_ids("e1", &[desc("a", DataType::F64), desc("a", DataType::Bool)])
            .unwrap_err();
        assert!(matches!(err, StoreError::Validation(_)));
    }

    #[test]
    fn point_id_cross_endpoint_isolation() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        s.create_endpoint(&ep("e2", "d1")).unwrap();
        let d1 = s
            .assign_point_ids("e1", &[desc("x", DataType::F64)])
            .unwrap();
        let d2 = s
            .assign_point_ids("e2", &[desc("x", DataType::F64)])
            .unwrap();
        // 同 key 跨 endpoint 独立分配，均从 1 起
        assert_eq!(d1[0].point_id, 1);
        assert_eq!(d2[0].point_id, 1);
    }
}
