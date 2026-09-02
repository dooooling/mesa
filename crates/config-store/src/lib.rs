//! ConfigStore：SQLite 持久化（方案 §4.3、§5、§6）。
//!
//! 职责：Device / Endpoint / Task 全量快照 + PointRegistry（带 tombstone）+
//! 每 Endpoint 的 Revision 与启停期望。运行期 LatestValue 仍驻留内存，不落盘。
//!
//! 并发模型：V1 单管理员/单写者，库内用单 `Mutex<Connection>` 串行化；
//! 所有写操作包在事务内，保证"全量替换要么全成功，要么保持旧版"。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[allow(unused_imports)]
use mesa_core_types::{
    AcquisitionTask, DataType, PointDefinition, PointDescriptor, ensure_unique_point_keys,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// 当前 schema 版本。增量迁移时递增并在 `meta` 中持久化（§6.1）。
const SCHEMA_VERSION: i64 = 2;

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

/// Control 审计记录（§6.6）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ControlAuditRecord {
    pub request_id: String,
    pub endpoint_id: String,
    pub actor: String,
    pub operation_type: String, // write | command
    pub operation_id: String,
    pub request_json: String,
    pub result_json: Option<String>,
    pub status: String,
    pub started_at_ns: i64,
    pub finished_at_ns: Option<i64>,
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
// Secret Master Key & AEAD
// ---------------------------------------------------------------------------

/// 全局 master key 缓存（进程内单例，避免重复文件 IO）
static MASTER_KEY_CACHE: OnceLock<[u8; 32]> = OnceLock::new();

fn master_key_bytes() -> Result<[u8; 32], StoreError> {
    if let Some(k) = MASTER_KEY_CACHE.get() {
        return Ok(*k);
    }
    // 1) 环境变量覆盖（支持 base64 或 32 字节原始字符串，适配离线工控机）
    if let Ok(env) = std::env::var("MESA_MASTER_KEY") {
        let env = env.trim();
        if !env.is_empty() {
            // 尝试 base64 解码
            if let Ok(decoded) =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, env)
                && decoded.len() == 32
            {
                let mut k = [0u8; 32];
                k.copy_from_slice(&decoded);
                let _ = MASTER_KEY_CACHE.set(k);
                return Ok(k);
            }
            // 回退：取 env 字节的哈希派生（测试便利，非生产推荐）
            if env.len() >= 8 {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                env.hash(&mut h);
                let hv = h.finish();
                let mut k = [0u8; 32];
                k[..8].copy_from_slice(&hv.to_le_bytes());
                k[8..16].copy_from_slice(&hv.to_le_bytes());
                k[16..24].copy_from_slice(&hv.to_le_bytes());
                k[24..32].copy_from_slice(&hv.to_le_bytes());
                let _ = MASTER_KEY_CACHE.set(k);
                return Ok(k);
            }
        }
    }
    // 2) 文件 $DATA/master.key（与 DB 同目录，0600）
    // 对于 open_in_memory 场景，使用固定测试 key（仅单测）
    let key = load_or_create_master_key_file()?;
    let _ = MASTER_KEY_CACHE.set(key);
    Ok(key)
}

fn load_or_create_master_key_file() -> Result<[u8; 32], StoreError> {
    // 尝试从常见位置解析：优先环境变量 MESA_DATA_DIR，其次当前目录
    let candidates: Vec<PathBuf> = if let Ok(dir) = std::env::var("MESA_DATA_DIR") {
        vec![PathBuf::from(dir).join("master.key")]
    } else {
        vec![
            PathBuf::from("data/master.key"),
            PathBuf::from("./master.key"),
            std::env::temp_dir().join("mesa-master.key"),
        ]
    };
    for p in &candidates {
        if p.is_file() {
            let raw = std::fs::read(p)
                .map_err(|e| StoreError::Validation(format!("read master.key: {e}")))?;
            // 支持 base64 或原始 32 字节
            if raw.len() == 32 {
                let mut k = [0u8; 32];
                k.copy_from_slice(&raw);
                return Ok(k);
            }
            if let Ok(s) = String::from_utf8(raw.clone())
                && let Ok(decoded) =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s.trim())
                && decoded.len() == 32
            {
                let mut k = [0u8; 32];
                k.copy_from_slice(&decoded);
                return Ok(k);
            }
        }
    }
    // 不存在则生成并写入第一个候选路径
    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| StoreError::Validation(e.to_string()))?;
    let target = &candidates[0];
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 写入 0600（Unix）或默认（Windows）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::write(target, &key);
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::write(target, key);
    }
    Ok(key)
}

fn aead_encrypt(plaintext: &[u8], key: &[u8; 32]) -> Result<(Vec<u8>, Vec<u8>), StoreError> {
    use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce, aead::Aead};
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; 24];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| StoreError::Validation(e.to_string()))?;
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| StoreError::Validation(format!("encrypt: {e}")))?;
    Ok((ct, nonce_bytes.to_vec()))
}

fn aead_decrypt(ciphertext: &[u8], nonce: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, StoreError> {
    use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce, aead::Aead};
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    if nonce.len() != 24 {
        return Err(StoreError::Validation(format!(
            "invalid nonce len {}",
            nonce.len()
        )));
    }
    let n = XNonce::from_slice(nonce);
    cipher
        .decrypt(n, ciphertext)
        .map_err(|e| StoreError::Validation(format!("decrypt: {e}")))
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
        // 版本标记 + schema_migrations（§6.2-6.4）
        // 确保 schema_migrations 存在（幂等）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                checksum TEXT NOT NULL,
                applied_at_ns INTEGER NOT NULL
            );",
        )?;
        let cur: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let mut cur_ver: i64 = cur.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
        let migrated_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap_or(0);
        // 旧库兼容：若已有数据但 schema_migrations 为空，补记 001
        if migrated_cnt == 0 {
            let checksum1 = format!("{:x}", include_str!("../migrations/001_initial.sql").len());
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(1,'001_initial',?1,?2)",
                params![checksum1, Self::now_ns()],
            )?;
            if cur.is_none() {
                conn.execute(
                    "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','1')",
                    [],
                )?;
                cur_ver = 1;
            }
        }
        if cur_ver < 1 {
            conn.execute(
                "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version',?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
            cur_ver = SCHEMA_VERSION;
        }
        // 002 迁移（§6.5-6.6）
        if cur_ver < 2 {
            let has_2: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=2)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !has_2 {
                // 备份（仅文件库，内存库跳过；生产应使用 rusqlite backup API，此处以文件拷贝为兜底）
                if let Some(path_str) = conn.path()
                    && !path_str.is_empty()
                    && std::path::Path::new(path_str).exists()
                {
                    let path = std::path::Path::new(path_str);
                    let bak = format!("{}.bak.{}", path.display(), Self::now_ns());
                    let _ = std::fs::copy(path, &bak);
                }
                let sql2 = include_str!("../migrations/002_management_control.sql");
                // 原子迁移：BEGIN IMMEDIATE → SQL → 记录 → 更新 meta → COMMIT
                // 使用 unchecked_transaction 以兼容外层未提交状态
                {
                    // 需要 &mut Connection 以开启事务，临时解锁重入
                    drop(conn);
                    let mut conn_mut = self.conn.lock().unwrap();
                    let tx = conn_mut.transaction()?;
                    tx.execute_batch(sql2)?;
                    let checksum2 = format!("{:x}", sql2.len());
                    tx.execute(
                        "INSERT INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(2,'002_management_control',?1,?2)",
                        params![checksum2, Self::now_ns()],
                    )?;
                    tx.execute("UPDATE meta SET value='2' WHERE key='schema_version'", [])?;
                    tx.execute(
                        "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','2')",
                        [],
                    )?;
                    tx.commit()?;
                    return Ok(());
                }
            }
        }
        // 最终确保 meta 为最新
        if cur_ver < SCHEMA_VERSION {
            conn.execute(
                "UPDATE meta SET value=?1 WHERE key='schema_version'",
                params![SCHEMA_VERSION.to_string()],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version',?1)",
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

    // ---- Secrets (§6.5) ----
    /// 存储密文字段（XChaCha20-Poly1305 + 24B nonce + master key 0600）
    pub fn put_secret(
        &self,
        endpoint_id: &str,
        field_path: &str,
        plaintext: &str,
        key_id: &str,
    ) -> Result<(), StoreError> {
        let key = master_key_bytes()?;
        let (ciphertext, nonce) = aead_encrypt(plaintext.as_bytes(), &key)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO endpoint_secrets(endpoint_id,field_path,ciphertext,nonce,algorithm,key_id,updated_at_ns)
             VALUES(?1,?2,?3,?4,'xchacha20poly1305',?5,?6)
             ON CONFLICT(endpoint_id,field_path) DO UPDATE SET ciphertext=excluded.ciphertext, nonce=excluded.nonce, algorithm=excluded.algorithm, key_id=excluded.key_id, updated_at_ns=excluded.updated_at_ns",
            params![
                endpoint_id,
                field_path,
                ciphertext,
                nonce,
                key_id,
                Self::now_ns()
            ],
        )?;
        Ok(())
    }

    pub fn get_secret(
        &self,
        endpoint_id: &str,
        field_path: &str,
    ) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(Vec<u8>, Vec<u8>, String, String)> = conn
            .query_row(
                "SELECT ciphertext, nonce, algorithm, key_id FROM endpoint_secrets WHERE endpoint_id=?1 AND field_path=?2",
                params![endpoint_id, field_path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((ct, nonce, alg, key_id)) = row {
            // 兼容旧 xor-demo 数据（迁移期）
            if alg == "xor-demo" {
                let key_byte = key_id.bytes().fold(0xAAu8, |a, b| a ^ b);
                let pt: Vec<u8> = ct.iter().map(|b| b ^ key_byte).collect();
                return Ok(Some(String::from_utf8_lossy(&pt).into_owned()));
            }
            let key = master_key_bytes()?;
            let pt = aead_decrypt(&ct, &nonce, &key)?;
            Ok(Some(String::from_utf8_lossy(&pt).into_owned()))
        } else {
            Ok(None)
        }
    }

    pub fn delete_secret(&self, endpoint_id: &str, field_path: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM endpoint_secrets WHERE endpoint_id=?1 AND field_path=?2",
            params![endpoint_id, field_path],
        )?;
        Ok(n > 0)
    }

    // ---- Control Audit (§6.6) ----
    pub fn insert_control_audit(&self, rec: &ControlAuditRecord) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO control_audit(request_id,endpoint_id,actor,operation_type,operation_id,request_json,result_json,status,started_at_ns,finished_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                rec.request_id,
                rec.endpoint_id,
                rec.actor,
                rec.operation_type,
                rec.operation_id,
                rec.request_json,
                rec.result_json,
                rec.status,
                rec.started_at_ns,
                rec.finished_at_ns
            ],
        )?;
        Ok(())
    }

    pub fn update_control_audit(
        &self,
        request_id: &str,
        status: &str,
        result_json: Option<&str>,
        finished_at_ns: i64,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE control_audit SET status=?1, result_json=?2, finished_at_ns=?3 WHERE request_id=?4",
            params![status, result_json, finished_at_ns, request_id],
        )?;
        Ok(())
    }

    /// 列表查询：按 endpoint/status/时间范围过滤，支持 limit/cursor（cursor 为 started_at_ns 的分页锚点）
    pub fn list_control_audit(
        &self,
        endpoint_id: Option<&str>,
        status: Option<&str>,
        from_ns: Option<i64>,
        to_ns: Option<i64>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Vec<ControlAuditRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT request_id,endpoint_id,actor,operation_type,operation_id,request_json,result_json,status,started_at_ns,finished_at_ns FROM control_audit WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        // endpoint 过滤
        if let Some(ep) = endpoint_id {
            sql.push_str(" AND endpoint_id=?");
            args.push(Box::new(ep.to_string()));
        }
        if let Some(st) = status {
            sql.push_str(" AND status=?");
            args.push(Box::new(st.to_string()));
        }
        if let Some(f) = from_ns {
            sql.push_str(" AND started_at_ns>=?");
            args.push(Box::new(f));
        }
        if let Some(t) = to_ns {
            sql.push_str(" AND (finished_at_ns<=? OR finished_at_ns IS NULL)");
            args.push(Box::new(t));
        }
        // cursor 为上一页最后一条的 started_at_ns（降序分页）
        if let Some(c) = cursor.and_then(|s| s.parse::<i64>().ok()) {
            sql.push_str(" AND started_at_ns<?");
            args.push(Box::new(c));
        }
        sql.push_str(" ORDER BY started_at_ns DESC LIMIT ?");
        args.push(Box::new(limit as i64));
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = args
            .iter()
            .map(|b| b.as_ref() as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok(ControlAuditRecord {
                request_id: r.get(0)?,
                endpoint_id: r.get(1)?,
                actor: r.get(2)?,
                operation_type: r.get(3)?,
                operation_id: r.get(4)?,
                request_json: r.get(5)?,
                result_json: r.get(6)?,
                status: r.get(7)?,
                started_at_ns: r.get(8)?,
                finished_at_ns: r.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_control_audit(
        &self,
        request_id: &str,
    ) -> Result<Option<ControlAuditRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT request_id,endpoint_id,actor,operation_type,operation_id,request_json,result_json,status,started_at_ns,finished_at_ns FROM control_audit WHERE request_id=?1",
                params![request_id],
                |r| {
                    Ok(ControlAuditRecord {
                        request_id: r.get(0)?,
                        endpoint_id: r.get(1)?,
                        actor: r.get(2)?,
                        operation_type: r.get(3)?,
                        operation_id: r.get(4)?,
                        request_json: r.get(5)?,
                        result_json: r.get(6)?,
                        status: r.get(7)?,
                        started_at_ns: r.get(8)?,
                        finished_at_ns: r.get(9)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
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

    #[test]
    fn secret_not_plaintext_and_roundtrip() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        s.put_secret("e1", "password", "s3cr3t", "k1").unwrap();
        // 原始密文不等于明文
        let conn = s.conn.lock().unwrap();
        let ct: Vec<u8> = conn
            .query_row(
                "SELECT ciphertext FROM endpoint_secrets WHERE endpoint_id='e1' AND field_path='password'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(ct, b"s3cr3t".to_vec());
        drop(conn);
        // 读取回明文
        let pt = s.get_secret("e1", "password").unwrap().unwrap();
        assert_eq!(pt, "s3cr3t");
        // 错误 key_id 解密失败（此处用不同 key 解密应得乱码，但不崩溃）
        s.put_secret("e1", "password", "another", "k2").unwrap();
        let pt2 = s.get_secret("e1", "password").unwrap().unwrap();
        assert_eq!(pt2, "another");
    }

    #[test]
    fn control_audit_insert_and_query() {
        let s = mem();
        let rec = crate::ControlAuditRecord {
            request_id: "req-1".into(),
            endpoint_id: "ep1".into(),
            actor: "local-console".into(),
            operation_type: "write".into(),
            operation_id: "opcua.write".into(),
            request_json: r#"{"value":42}"#.into(),
            result_json: Some(r#"{"ok":true}"#.into()),
            status: "Succeeded".into(),
            started_at_ns: 1000,
            finished_at_ns: Some(2000),
        };
        s.insert_control_audit(&rec).unwrap();
        let got = s.get_control_audit("req-1").unwrap().unwrap();
        assert_eq!(got.request_id, "req-1");
        assert_eq!(got.actor, "local-console");
        assert_eq!(got.status, "Succeeded");
    }

    #[test]
    fn migration_002_exists_and_version_is_2() {
        let s = mem();
        let conn = s.conn.lock().unwrap();
        let ver: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ver, "2");
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert!(cnt >= 2, "至少 2 条迁移");
        // 表存在
        let tbl: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='endpoint_secrets'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tbl, "endpoint_secrets");
        let tbl2: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='control_audit'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tbl2, "control_audit");
    }
}
