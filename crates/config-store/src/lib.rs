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
    AcquisitionTask, DataType, EventTask, PointDefinition, PointDescriptor,
    ensure_unique_point_keys,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// 当前 schema 版本。增量迁移时递增并在 `meta` 中持久化（§6.1）。
const SCHEMA_VERSION: i64 = 7;

// ---------------------------------------------------------------------------
// 记录类型
// ---------------------------------------------------------------------------

/// Device 记录（§5.2）：用户管理的设备/机器/采集对象容器（非严格物理实体）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
}

/// Endpoint 记录（§5.3）。`connection` 的语义由 Driver 解释，Core 只做 JSON 透传。
/// `name` 为展示名（PR25）：创建必填、更新可改；`driver_id` 创建后不可变。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EndpointRecord {
    pub id: String,
    pub name: String,
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

/// P0-1 Final：key 目录由 `open(path)` 的 DB 父目录决定（docstring 本就承诺
/// "与 DB 同目录"），`open_in_memory` 对应 `None`。CWD 相对路径
/// （`data/master.key` 等）已删除——`cargo test -p mesa-config-store` 的
/// CWD 恰是 crate 目录，旧实现会在源码树里生成真 key 并被误提交。
/// 缓存是实例级（`ConfigStore::master_key`），禁止进程全局 static——同一进程
/// 内 in-memory 库（固定测试 key）与多个文件库（各目录独立 key）共存时，
/// 全局缓存会把先访问者的 key 串给后访问者（文件库 key 文件甚至不生成，
/// 进程重启后旧 Secret 永久无法解密）。
fn load_master_key(key_dir: Option<&PathBuf>) -> Result<[u8; 32], StoreError> {
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
                return Ok(k);
            }
            // 生产级：仅接受 base64 32 字节，其余一律拒绝（禁止弱回退）
            return Err(StoreError::Validation(
                "MESA_MASTER_KEY 无效：需为 base64 编码的 32 字节随机密钥".into(),
            ));
        }
    }
    // 2) in-memory 库：固定测试 key，不碰磁盘（单测永不生成真 key 文件）
    let Some(dir) = key_dir else {
        return Ok([0xA5u8; 32]);
    };
    // 3) 文件库：`MESA_DATA_DIR` 覆盖，否则与 DB 同目录，0600
    let target = if let Ok(env_dir) = std::env::var("MESA_DATA_DIR") {
        PathBuf::from(env_dir).join("master.key")
    } else {
        dir.join("master.key")
    };
    load_or_create_master_key_file(&target)
}

fn load_or_create_master_key_file(target: &PathBuf) -> Result<[u8; 32], StoreError> {
    if target.is_file() {
        let raw = std::fs::read(target)
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
    // 不存在则生成并写入目标路径
    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| StoreError::Validation(e.to_string()))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| StoreError::Validation(format!("create master.key dir: {e}")))?;
    }
    // fail-closed：临时文件 + fsync + 0600 + 原子重命名，避免“加密成功但密钥未落盘”导致重启后永久无法解密
    let tmp = target.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| StoreError::Validation(format!("create master.key tmp: {e}")))?;
        f.write_all(&key)
            .map_err(|e| StoreError::Validation(format!("write master.key tmp: {e}")))?;
        f.sync_all()
            .map_err(|e| StoreError::Validation(format!("fsync master.key tmp: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| StoreError::Validation(format!("chmod master.key tmp: {e}")))?;
        }
    }
    std::fs::rename(&tmp, target)
        .map_err(|e| StoreError::Validation(format!("rename master.key: {e}")))?;
    // 确保父目录落盘（尽力）
    if let Some(parent) = target.parent()
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
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

/// point_registry 行（诊断用）：key/id/类型/墓碑/来源标签。
pub type RegistryRow = (String, u32, String, bool, Option<String>);

pub struct ConfigStore {
    conn: Mutex<Connection>,
    /// master.key 目录：文件库 = DB 父目录（P0-1），内存库 = None（固定测试 key）。
    key_dir: Option<PathBuf>,
    /// master key 实例级缓存（P0-1 Final）：`get_or_try_init` 串行化并发首次
    /// 访问；各实例独立，in-memory/多目录文件库同进程共存不串 key。
    master_key: OnceLock<[u8; 32]>,
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
        // P0-1：key 目录锚定 DB 位置，不再依赖进程 CWD。
        // 无父目录（如 `mesa.db`）即当前目录——绝不能回落到测试 key。
        let key_dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => Some(p.to_path_buf()),
            _ => Some(PathBuf::from(".")),
        };
        let s = Self {
            conn: Mutex::new(conn),
            key_dir,
            master_key: OnceLock::new(),
        };
        s.migrate()?;
        Ok(s)
    }

    /// 内存库（单测/临时使用）：Secret 用固定测试 key，不写任何 key 文件。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let s = Self {
            conn: Mutex::new(conn),
            key_dir: None,
            master_key: OnceLock::new(),
        };
        s.migrate()?;
        Ok(s)
    }

    /// 本实例的 master key（P0-1 Final：实例级缓存 + 目录锚定 DB，跨实例
    /// desync 不可能）。并发首次访问由初始化锁串行化——同实例双生成会产生
    /// "缓存 key ≠ 落盘 key"；锁是进程共享的，但只保护初始化路径，缓存仍
    /// 是实例级（`OnceLock::get_or_try_init` 仍为 nightly 特性，不可用）。
    fn master_key_bytes(&self) -> Result<[u8; 32], StoreError> {
        if let Some(k) = self.master_key.get() {
            return Ok(*k);
        }
        static INIT_LOCK: Mutex<()> = Mutex::new(());
        let _g = INIT_LOCK.lock().unwrap();
        if let Some(k) = self.master_key.get() {
            return Ok(*k);
        }
        let key = load_master_key(self.key_dir.as_ref())?;
        let _ = self.master_key.set(key);
        Ok(key)
    }

    /// 迁移前的文件拷贝兜底（仅文件库；生产应使用 rusqlite backup API）。
    /// 调用方需持有 conn guard（只读 path，不重入加锁）。
    fn backup_file_db(conn: &Connection) {
        if let Some(path_str) = conn.path()
            && !path_str.is_empty()
            && std::path::Path::new(path_str).exists()
        {
            let path = std::path::Path::new(path_str);
            let bak = format!("{}.bak.{}", path.display(), Self::now_ns());
            let _ = std::fs::copy(path, &bak);
        }
    }

    fn migrate(&self) -> Result<(), StoreError> {
        // P0-1：单个 guard 走完全程，禁止中途 drop/re-lock——旧代码在 002
        // 分支 drop 外层 guard，而现实 v2 库根本不进 <2 分支，003 的重入
        // lock 在同一线程永久自锁。conn.transaction() 只需 &mut 重借，
        // 不需要释放 guard。
        let mut conn = self.conn.lock().unwrap();
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
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS endpoints(
                id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
                driver_id TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
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
                source_label TEXT NULL,
                PRIMARY KEY(endpoint_id, point_key)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_point_registry_ep_pid
                ON point_registry(endpoint_id, point_id);
            CREATE TABLE IF NOT EXISTS config_revision(
                endpoint_id TEXT PRIMARY KEY REFERENCES endpoints(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS bootstrap_idempotency(
                key TEXT PRIMARY KEY,
                request_hash TEXT NOT NULL,
                device_id TEXT NOT NULL,
                endpoint_id TEXT NOT NULL,
                result_json TEXT NOT NULL,
                created_at_ns INTEGER NOT NULL
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
                Self::backup_file_db(&conn);
                let sql2 = include_str!("../migrations/002_management_control.sql");
                // 原子迁移：SQL → 记录 → 更新 meta → COMMIT（单事务）
                let tx = conn.transaction()?;
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
                cur_ver = 2;
            }
        }
        // 003 迁移（PR7 EventTask，v1.1 §10）
        if cur_ver < 3 {
            let has_3: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=3)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !has_3 {
                Self::backup_file_db(&conn);
                let sql3 = include_str!("../migrations/003_event_tasks.sql");
                let tx = conn.transaction()?;
                tx.execute_batch(sql3)?;
                let checksum3 = format!("{:x}", sql3.len());
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(3,'003_event_tasks',?1,?2)",
                    params![checksum3, Self::now_ns()],
                )?;
                tx.execute("UPDATE meta SET value='3' WHERE key='schema_version'", [])?;
                tx.execute(
                    "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','3')",
                    [],
                )?;
                tx.commit()?;
                cur_ver = 3;
            }
        }
        // 004 迁移（DeviceProfile 整条链删除）：去掉 devices.profile。
        // v4 不变量：所有 v4 库的 devices 表严格为 (id, name)。
        if cur_ver < 4 {
            let has_4: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=4)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !has_4 {
                Self::backup_file_db(&conn);
                // 新库建表已不含 profile 列，直接 DROP 会报错；先查 PRAGMA。
                let has_col: bool = conn
                    .prepare("PRAGMA table_info(devices)")?
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?
                    .iter()
                    .any(|c| c == "profile");
                let tx = conn.transaction()?;
                if has_col {
                    let sql4 = include_str!("../migrations/004_remove_device_profile.sql");
                    tx.execute_batch(sql4)?;
                }
                let checksum4 = format!(
                    "{:x}",
                    include_str!("../migrations/004_remove_device_profile.sql").len()
                );
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(4,'004_remove_device_profile',?1,?2)",
                    params![checksum4, Self::now_ns()],
                )?;
                tx.execute("UPDATE meta SET value='4' WHERE key='schema_version'", [])?;
                tx.execute(
                    "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','4')",
                    [],
                )?;
                tx.commit()?;
                cur_ver = 4;
            }
        }
        // 005 迁移（PR25）：endpoints 增加展示名 name。
        // v5 不变量：所有 v5 库的 endpoints 表严格含 name 列（旧行默认为 ''）。
        if cur_ver < 5 {
            let has_5: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=5)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !has_5 {
                Self::backup_file_db(&conn);
                // 新库建表已含 name 列，直接 ADD 会报错；先查 PRAGMA。
                let has_col: bool = conn
                    .prepare("PRAGMA table_info(endpoints)")?
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?
                    .iter()
                    .any(|c| c == "name");
                let tx = conn.transaction()?;
                if !has_col {
                    let sql5 = include_str!("../migrations/005_endpoint_name.sql");
                    tx.execute_batch(sql5)?;
                }
                // 回填无条件执行：列已存在但 migration record 缺失的重入路径
                // 同样保证 v5 不变量（所有 name 非空，旧行以 id 回填）。
                tx.execute("UPDATE endpoints SET name=id WHERE trim(name)=''", [])?;
                let checksum5 = format!(
                    "{:x}",
                    include_str!("../migrations/005_endpoint_name.sql").len()
                );
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(5,'005_endpoint_name',?1,?2)",
                    params![checksum5, Self::now_ns()],
                )?;
                tx.execute("UPDATE meta SET value='5' WHERE key='schema_version'", [])?;
                tx.execute(
                    "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','5')",
                    [],
                )?;
                tx.commit()?;
                cur_ver = 5;
            }
        }
        // 006 迁移（M5 device-bootstrap）：幂等键表。新库建表已含该表；
        // 旧库走迁移补建（IF NOT EXISTS，重入安全）。
        if cur_ver < 6 {
            let has_6: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=6)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !has_6 {
                Self::backup_file_db(&conn);
                let sql6 = include_str!("../migrations/006_bootstrap_idempotency.sql");
                let tx = conn.transaction()?;
                tx.execute_batch(sql6)?;
                let checksum6 = format!("{:x}", sql6.len());
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(6,'006_bootstrap_idempotency',?1,?2)",
                    params![checksum6, Self::now_ns()],
                )?;
                tx.execute("UPDATE meta SET value='6' WHERE key='schema_version'", [])?;
                tx.execute(
                    "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','6')",
                    [],
                )?;
                tx.commit()?;
                cur_ver = 6;
            }
        }
        // 007 迁移（Point Presentation Metadata P1）：point_registry 新增
        // source_label（纯展示，非 identity）。新库建表已含该列；旧库走迁移
        // 补列（ADD COLUMN，重入安全：列已存在即跳过）。
        if cur_ver < 7 {
            let has_7: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=7)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !has_7 {
                Self::backup_file_db(&conn);
                let has_col: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('point_registry') WHERE name='source_label')",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(false);
                let sql7 = include_str!("../migrations/007_point_source_label.sql");
                let tx = conn.transaction()?;
                if !has_col {
                    tx.execute_batch(sql7)?;
                }
                let checksum7 = format!("{:x}", sql7.len());
                tx.execute(
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at_ns) VALUES(7,'007_point_source_label',?1,?2)",
                    params![checksum7, Self::now_ns()],
                )?;
                tx.execute("UPDATE meta SET value='7' WHERE key='schema_version'", [])?;
                tx.execute(
                    "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version','7')",
                    [],
                )?;
                tx.commit()?;
                cur_ver = 7;
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
            "INSERT INTO devices(id,name) VALUES(?1,?2)",
            params![rec.id, rec.name],
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
        let mut stmt = conn.prepare("SELECT id,name FROM devices ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(DeviceRecord {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_device(&self, id: &str) -> Result<Option<DeviceRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,name FROM devices WHERE id=?1",
            params![id],
            |r| {
                Ok(DeviceRecord {
                    id: r.get(0)?,
                    name: r.get(1)?,
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
            "UPDATE devices SET name=?1 WHERE id=?2",
            params![rec.name, rec.id],
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
        Self::validate_endpoint_name(&rec.name)?;
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
            "INSERT INTO endpoints(id,device_id,driver_id,name,connection_json,desired_running,updated_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![rec.id, rec.device_id, rec.driver_id, rec.name, rec.connection_json, rec.desired_running as i32, rec.updated_at_ns],
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

    /// 原子创建 Endpoint + Secrets（单事务，避免 FK 与半提交）
    pub fn create_endpoint_with_secrets(
        &self,
        rec: &EndpointRecord,
        secrets: &[(String, String)],
    ) -> Result<(), StoreError> {
        Self::validate_id(&rec.id)?;
        Self::validate_id(&rec.device_id)?;
        Self::validate_id(&rec.driver_id)?;
        Self::validate_endpoint_name(&rec.name)?;
        let v: serde_json::Value =
            serde_json::from_str(&rec.connection_json).map_err(StoreError::Json)?;
        if !v.is_object() {
            return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
        }
        // 预先加密所有 secrets，避免事务中途失败；无 Secret 时不初始化 master key
        let mut encs: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
        if !secrets.is_empty() {
            let key = self.master_key_bytes()?;
            for (field, pt) in secrets {
                let (ct, nonce) = aead_encrypt(pt.as_bytes(), &key)?;
                encs.push((field.clone(), ct, nonce));
            }
        }
        let mut conn = self.conn.lock().unwrap();
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
        let tx = conn.transaction()?;
        let n = tx.execute(
            "INSERT INTO endpoints(id,device_id,driver_id,name,connection_json,desired_running,updated_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![rec.id, rec.device_id, rec.driver_id, rec.name, rec.connection_json, rec.desired_running as i32, rec.updated_at_ns],
        );
        match n {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(StoreError::Duplicate(format!(
                    "endpoint `{}` 已存在",
                    rec.id
                )));
            }
            Err(e) => return Err(e.into()),
        }
        tx.execute(
            "INSERT OR IGNORE INTO config_revision(endpoint_id,revision) VALUES(?1,0)",
            params![rec.id],
        )?;
        for (field, ct, nonce) in encs {
            tx.execute(
                "INSERT INTO endpoint_secrets(endpoint_id,field_path,ciphertext,nonce,algorithm,key_id,updated_at_ns)
                 VALUES(?1,?2,?3,?4,'xchacha20poly1305','master',?5)
                 ON CONFLICT(endpoint_id,field_path) DO UPDATE SET ciphertext=excluded.ciphertext, nonce=excluded.nonce, algorithm=excluded.algorithm, key_id=excluded.key_id, updated_at_ns=excluded.updated_at_ns",
                params![rec.id, field, ct, nonce, Self::now_ns()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 原子更新 Endpoint + Secrets（单事务）
    pub fn update_endpoint_with_secrets(
        &self,
        rec: &EndpointRecord,
        secrets_to_upsert: &[(String, String)],
        secrets_to_delete: &[String],
    ) -> Result<bool, StoreError> {
        Self::validate_endpoint_name(&rec.name)?;
        let v: serde_json::Value =
            serde_json::from_str(&rec.connection_json).map_err(StoreError::Json)?;
        if !v.is_object() {
            return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
        }
        let mut encs: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
        if !secrets_to_upsert.is_empty() {
            let key = self.master_key_bytes()?;
            for (field, pt) in secrets_to_upsert {
                let (ct, nonce) = aead_encrypt(pt.as_bytes(), &key)?;
                encs.push((field.clone(), ct, nonce));
            }
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        // driver_id 创建后不可变：UPDATE 永不触碰 driver_id（API 层已拒绝变更）
        let n = tx.execute(
            "UPDATE endpoints SET device_id=?1, name=?2, connection_json=?3, desired_running=?4, updated_at_ns=?5 WHERE id=?6",
            params![rec.device_id, rec.name, rec.connection_json, rec.desired_running as i32, rec.updated_at_ns, rec.id],
        )?;
        if n == 0 {
            return Ok(false);
        }
        for (field, ct, nonce) in encs {
            tx.execute(
                "INSERT INTO endpoint_secrets(endpoint_id,field_path,ciphertext,nonce,algorithm,key_id,updated_at_ns)
                 VALUES(?1,?2,?3,?4,'xchacha20poly1305','master',?5)
                 ON CONFLICT(endpoint_id,field_path) DO UPDATE SET ciphertext=excluded.ciphertext, nonce=excluded.nonce, algorithm=excluded.algorithm, key_id=excluded.key_id, updated_at_ns=excluded.updated_at_ns",
                params![rec.id, field, ct, nonce, Self::now_ns()],
            )?;
        }
        for field in secrets_to_delete {
            tx.execute(
                "DELETE FROM endpoint_secrets WHERE endpoint_id=?1 AND field_path=?2",
                params![rec.id, field],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn list_endpoints(&self) -> Result<Vec<EndpointRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,device_id,driver_id,name,connection_json,desired_running,updated_at_ns FROM endpoints ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(EndpointRecord {
                id: r.get(0)?,
                device_id: r.get(1)?,
                driver_id: r.get(2)?,
                name: r.get(3)?,
                connection_json: r.get(4)?,
                desired_running: r.get::<_, i32>(5)? != 0,
                updated_at_ns: r.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_endpoint(&self, id: &str) -> Result<Option<EndpointRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,device_id,driver_id,name,connection_json,desired_running,updated_at_ns FROM endpoints WHERE id=?1",
            params![id],
            |r| {
                Ok(EndpointRecord {
                    id: r.get(0)?,
                    device_id: r.get(1)?,
                    driver_id: r.get(2)?,
                    name: r.get(3)?,
                    connection_json: r.get(4)?,
                    desired_running: r.get::<_, i32>(5)? != 0,
                    updated_at_ns: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_endpoint(&self, rec: &EndpointRecord) -> Result<bool, StoreError> {
        Self::validate_endpoint_name(&rec.name)?;
        let v: serde_json::Value =
            serde_json::from_str(&rec.connection_json).map_err(StoreError::Json)?;
        if !v.is_object() {
            return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
        }
        let conn = self.conn.lock().unwrap();
        // driver_id 创建后不可变：UPDATE 永不触碰 driver_id
        let n = conn.execute(
            "UPDATE endpoints SET device_id=?1, name=?2, connection_json=?3, desired_running=?4, updated_at_ns=?5 WHERE id=?6",
            params![rec.device_id, rec.name, rec.connection_json, rec.desired_running as i32, rec.updated_at_ns, rec.id],
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

    // ---- Bootstrap（M5 device-bootstrap 原子事务）----
    //
    // 单事务完成 Device + Endpoint(+Secrets) + Tasks + revision：
    // 要么全部落库，要么全部回滚，不存在“device 建了但 endpoint 没建”的半提交。
    // Driver start 是 runtime side effect，不在 DB 事务内——由 API 层在事务
    // 提交后执行，失败走补偿删除（endpoint → device）。幂等键见下组函数。

    /// 原子 bootstrap：device + endpoint(with secrets) + tasks + revision。
    /// 调用前 secrets 必须已加密（`&[(field, ct, nonce)]`），本函数只负责落库。
    /// 成功返回新 revision。
    #[allow(clippy::too_many_arguments)]
    pub fn bootstrap_device_tx(
        &self,
        device: &DeviceRecord,
        endpoint: &EndpointRecord,
        secrets_enc: &[(String, Vec<u8>, Vec<u8>)],
        tasks: &[AcquisitionTask],
    ) -> Result<u64, StoreError> {
        Self::validate_id(&device.id)?;
        if device.name.trim().is_empty() {
            return Err(StoreError::Validation("device name 不能为空".into()));
        }
        Self::validate_id(&endpoint.id)?;
        Self::validate_id(&endpoint.device_id)?;
        Self::validate_id(&endpoint.driver_id)?;
        Self::validate_endpoint_name(&endpoint.name)?;
        let v: serde_json::Value =
            serde_json::from_str(&endpoint.connection_json).map_err(StoreError::Json)?;
        if !v.is_object() {
            return Err(StoreError::Validation("connection 必须为 JSON 对象".into()));
        }
        for t in tasks {
            t.validate()
                .map_err(|e| StoreError::Validation(e.to_string()))?;
        }
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
        let tx = conn.transaction()?;
        // device（重复即 Duplicate，由 API 层转 409）
        let n = tx.execute(
            "INSERT INTO devices(id,name) VALUES(?1,?2)",
            params![device.id, device.name],
        );
        match n {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(StoreError::Duplicate(format!(
                    "device `{}` 已存在",
                    device.id
                )));
            }
            Err(e) => return Err(e.into()),
        }
        // endpoint（device FK 由 SQLite 强制；重复即 Duplicate）
        let n = tx.execute(
            "INSERT INTO endpoints(id,device_id,driver_id,name,connection_json,desired_running,updated_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![endpoint.id, endpoint.device_id, endpoint.driver_id, endpoint.name, endpoint.connection_json, endpoint.desired_running as i32, endpoint.updated_at_ns],
        );
        match n {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(StoreError::Duplicate(format!(
                    "endpoint `{}` 已存在",
                    endpoint.id
                )));
            }
            Err(e) => return Err(e.into()),
        }
        tx.execute(
            "INSERT OR IGNORE INTO config_revision(endpoint_id,revision) VALUES(?1,0)",
            params![endpoint.id],
        )?;
        for (field, ct, nonce) in secrets_enc {
            tx.execute(
                "INSERT INTO endpoint_secrets(endpoint_id,field_path,ciphertext,nonce,algorithm,key_id,updated_at_ns)
                 VALUES(?1,?2,?3,?4,'xchacha20poly1305','master',?5)
                 ON CONFLICT(endpoint_id,field_path) DO UPDATE SET ciphertext=excluded.ciphertext, nonce=excluded.nonce, algorithm=excluded.algorithm, key_id=excluded.key_id, updated_at_ns=excluded.updated_at_ns",
                params![endpoint.id, field, ct, nonce, Self::now_ns()],
            )?;
        }
        Self::replace_tasks_in_tx(&tx, &endpoint.id, tasks)?;
        // bump revision（新 endpoint 从 0 → 1）
        let cur: i64 = tx
            .query_row(
                "SELECT revision FROM config_revision WHERE endpoint_id=?1",
                params![endpoint.id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let next = (cur + 1) as u64;
        tx.execute(
            "INSERT INTO config_revision(endpoint_id,revision) VALUES(?1,?2)
             ON CONFLICT(endpoint_id) DO UPDATE SET revision=excluded.revision",
            params![endpoint.id, next as i64],
        )?;
        tx.commit()?;
        Ok(next)
    }

    /// 原子 bootstrap（明文 secrets 版）：内部先加密再调单事务入口，
    /// 与 create_endpoint_with_secrets 同语义（无 Secret 时不初始化 master key）。
    pub fn bootstrap_device_tx_with_plaintext(
        &self,
        device: &DeviceRecord,
        endpoint: &EndpointRecord,
        secrets_plain: &[(String, String)],
        tasks: &[AcquisitionTask],
    ) -> Result<u64, StoreError> {
        let mut encs: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
        if !secrets_plain.is_empty() {
            let key = self.master_key_bytes()?;
            for (field, pt) in secrets_plain {
                let (ct, nonce) = aead_encrypt(pt.as_bytes(), &key)?;
                encs.push((field.clone(), ct, nonce));
            }
        }
        self.bootstrap_device_tx(device, endpoint, &encs, tasks)
    }

    /// 补偿删除：bootstrap 的 start side effect 失败时调用。
    /// endpoint → device 顺序删除（device 有 RESTRICT，顺序不可反）；
    /// endpoint 已不存在视为补偿成功（幂等删除）。
    pub fn bootstrap_compensate(
        &self,
        device_id: &str,
        endpoint_id: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM endpoints WHERE id=?1", params![endpoint_id])?;
        conn.execute("DELETE FROM devices WHERE id=?1", params![device_id])?;
        Ok(())
    }

    /// 查幂等记录：Ok(Some((request_hash, result_json))) = 同 key 已有结果。
    pub fn bootstrap_idempotency_get(
        &self,
        key: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT request_hash,result_json FROM bootstrap_idempotency WHERE key=?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    /// 写幂等记录（成功 bootstrap 后调用；INSERT OR REPLACE 覆盖同 key 同指纹重放）。
    pub fn bootstrap_idempotency_put(
        &self,
        key: &str,
        request_hash: &str,
        device_id: &str,
        endpoint_id: &str,
        result_json: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO bootstrap_idempotency(key,request_hash,device_id,endpoint_id,result_json,created_at_ns)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![key, request_hash, device_id, endpoint_id, result_json, Self::now_ns()],
        )?;
        Ok(())
    }

    /// 按 device 清理幂等记录（delete_device 第二层防御：防表永久增长 +
    /// 旧 key 残留。主防御是前端 per-operation key）。
    pub fn bootstrap_idempotency_delete_by_device(
        &self,
        device_id: &str,
    ) -> Result<u64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM bootstrap_idempotency WHERE device_id=?1",
            params![device_id],
        )?;
        Ok(n as u64)
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

    // ---- EventTasks（全量快照替换，与 Tasks 对等，PR7 v1.1 §10）----

    /// 全量替换某 endpoint 的事件任务集合。空数组表示清空（= 无事件订阅）。
    /// 成功时 revision 自增并返回新 revision（与 Data tasks 共用同一计数器：
    /// 任何配置变化都使 revision 前进）；失败则事务回滚、旧配置保持不变。
    /// 只做 Mesa contract/结构校验（id 非空唯一、Poll/interval 关系）；
    /// protocol-specific binding 语义留给 Driver 在 ConfigureEventTasks 时判定。
    pub fn replace_event_tasks(
        &self,
        endpoint_id: &str,
        tasks: &[EventTask],
    ) -> Result<u64, StoreError> {
        for t in tasks {
            t.validate()
                .map_err(|e| StoreError::Validation(e.to_string()))?;
        }
        {
            let mut seen = HashSet::new();
            for t in tasks {
                if !seen.insert(&t.id) {
                    return Err(StoreError::Validation(format!(
                        "duplicate event task id `{}`",
                        t.id
                    )));
                }
            }
        }

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
        tx.execute(
            "DELETE FROM event_tasks WHERE endpoint_id=?1",
            params![endpoint_id],
        )?;
        for t in tasks {
            tx.execute(
                "INSERT INTO event_tasks(endpoint_id,id,mode,interval_ms,binding_kind,binding_config_json)
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
        // bump revision（与 replace_tasks 同一计数器）
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

    pub fn list_event_tasks(&self, endpoint_id: &str) -> Result<Vec<EventTask>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,mode,interval_ms,binding_kind,binding_config_json FROM event_tasks WHERE endpoint_id=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![endpoint_id], |r| {
            let mode_s: String = r.get(1)?;
            // 新表由本函数写入，mode 取值受控；未知值视为数据损坏，硬失败
            // （不像老 tasks 表那样静默回落 Poll）。
            let mode = match mode_s.as_str() {
                "poll" => mesa_core_types::TaskMode::Poll,
                "subscribe" => mesa_core_types::TaskMode::Subscribe,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        format!("unknown event task mode `{other}`").into(),
                    ));
                }
            };
            let binding_config_json: String = r.get(4)?;
            // P0-2：坏 binding JSON 硬失败（与非法 mode 同语义；静默 `{}` 会
            // 让损坏的订阅变成"配了但行为不对"的幽灵任务）。
            let cfg: serde_json::Value =
                serde_json::from_str(&binding_config_json).map_err(|e: serde_json::Error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        format!("corrupt event binding_config_json: {e}").into(),
                    )
                })?;
            Ok(EventTask {
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
            // upsert：复用或新增均写入最新类型/unit/source_label 并清除 deleted。
            // source_label 是 configure 真值快照：Some 覆盖、None 清 NULL
            //（不得保留旧值，否则留下假来源）。
            tx.execute(
                "INSERT INTO point_registry(endpoint_id,point_key,point_id,data_type,unit,deleted,source_label)
                 VALUES(?1,?2,?3,?4,?5,0,?6)
                 ON CONFLICT(endpoint_id,point_key) DO UPDATE SET
                    point_id=excluded.point_id, data_type=excluded.data_type,
                    unit=excluded.unit, deleted=0, source_label=excluded.source_label",
                params![
                    endpoint_id,
                    d.point_key,
                    pid as i64,
                    d.data_type.as_str(),
                    d.unit,
                    d.source_label
                ],
            )?;
            out.push(PointDefinition {
                point_id: pid,
                point_key: d.point_key.clone(),
                data_type: d.data_type,
                unit: d.unit.clone(),
                source_label: d.source_label.clone(),
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
    pub fn point_registry_all(&self, endpoint_id: &str) -> Result<Vec<RegistryRow>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT point_key, point_id, data_type, deleted, source_label FROM point_registry WHERE endpoint_id=?1 ORDER BY point_id",
        )?;
        let rows = stmt.query_map(params![endpoint_id], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, i64>(1)? as u32,
                r.get::<_, String>(2)?,
                r.get::<_, i32>(3)? != 0,
                r.get::<_, Option<String>>(4)?,
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
        let key = self.master_key_bytes()?;
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
            let key = self.master_key_bytes()?;
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

    pub fn list_secret_fields(&self, endpoint_id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT field_path FROM endpoint_secrets WHERE endpoint_id=?1")?;
        let rows = stmt.query_map(params![endpoint_id], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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

    /// Endpoint 展示名校验（PR25）：非空（去空白后），长度 ≤128（与 id 同口径）。
    /// v5 不变量：所有行 name 非空（旧行迁移时以 id 回填）。
    fn validate_endpoint_name(name: &str) -> Result<(), StoreError> {
        if name.trim().is_empty() {
            return Err(StoreError::Validation("endpoint name 不能为空".into()));
        }
        if name.len() > 128 {
            return Err(StoreError::Validation("endpoint name 过长（≤128）".into()));
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
        }
    }

    fn ep(id: &str, device: &str) -> EndpointRecord {
        EndpointRecord {
            id: id.into(),
            name: format!("{id} 名称"),
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
            source_label: None,
        }
    }

    fn desc_label(key: &str, ty: DataType, label: Option<&str>) -> PointDescriptor {
        PointDescriptor {
            point_key: key.into(),
            data_type: ty,
            unit: None,
            source_label: label.map(|s| s.to_string()),
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

    fn event_task(id: &str) -> EventTask {
        EventTask {
            id: id.into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: "simulator.events".into(),
                config: serde_json::json!({"stream": "sim.events.alarm-cycle"}),
            },
        }
    }

    /// migration 链：新库 schema_version=最新版且 event_tasks 表可用；
    /// v2 无损（老数据路径不受影响由 002 测试覆盖，此处断言版本标记）。
    #[test]
    fn migration_003_event_tasks_table() {
        let s = mem();
        let ver: String = s
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ver, "7");
        let has: bool = s
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=3)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(has);
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        // 空快照即无订阅
        assert!(s.list_event_tasks("e1").unwrap().is_empty());
        let r1 = s.replace_event_tasks("e1", &[event_task("al")]).unwrap();
        assert_eq!(r1, 1);
        let tasks = s.list_event_tasks("e1").unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "al");
        assert_eq!(tasks[0].mode, TaskMode::Subscribe);
        // revision 与 Data tasks 共用计数器
        s.replace_tasks("e1", &[task("t1", 100)]).unwrap();
        assert_eq!(s.current_revision("e1").unwrap(), 2);
        // 全量替换 + 非法拒绝（Poll 无 interval）+ 回滚不推进 revision
        assert!(matches!(
            s.replace_event_tasks(
                "e1",
                &[EventTask {
                    id: "bad".into(),
                    mode: TaskMode::Poll,
                    interval_ms: None,
                    binding: DriverBinding {
                        kind: "k".into(),
                        config: serde_json::json!({}),
                    },
                }]
            ),
            Err(StoreError::Validation(_))
        ));
        assert_eq!(s.current_revision("e1").unwrap(), 2);
        assert_eq!(s.list_event_tasks("e1").unwrap().len(), 1);
        // 重复 id 拒绝
        assert!(matches!(
            s.replace_event_tasks("e1", &[event_task("d"), event_task("d")]),
            Err(StoreError::Validation(_))
        ));
        // endpoint 删除级联清理配置行（历史在 events.db，不受影响）
        assert!(s.delete_endpoint("e1").unwrap());
        assert!(s.list_event_tasks("e1").unwrap().is_empty());
    }

    /// P0-1 回归：现实 v2 文件库 open() 必须一次走到最新版（旧代码在此自锁）。
    /// 构造方式：按 v2 应有形态手写建表 + meta=2 + migrations 1,2 + 业务行
    ///（含带 profile 值的设备行，验证 004 只去列不丢行），
    /// 再 ConfigStore::open()（同一线程重复 lock 即永挂，测试会直接卡死）。
    #[test]
    fn v2_file_db_upgrades_to_v3_without_data_loss() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mesa-config-v2up-{}-{}.db",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                PRAGMA journal_mode=WAL;
                CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT NOT NULL, profile TEXT);
                CREATE TABLE endpoints(
                    id TEXT PRIMARY KEY,
                    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
                    driver_id TEXT NOT NULL,
                    connection_json TEXT NOT NULL,
                    desired_running INTEGER NOT NULL DEFAULT 0,
                    updated_at_ns INTEGER NOT NULL
                );
                CREATE TABLE tasks(
                    endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
                    id TEXT NOT NULL,
                    mode TEXT NOT NULL,
                    interval_ms INTEGER,
                    binding_kind TEXT NOT NULL,
                    binding_config_json TEXT NOT NULL,
                    PRIMARY KEY(endpoint_id, id)
                );
                CREATE TABLE schema_migrations(
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    applied_at_ns INTEGER NOT NULL
                );
                CREATE TABLE endpoint_secrets(
                    endpoint_id TEXT NOT NULL,
                    field_path TEXT NOT NULL,
                    ciphertext BLOB NOT NULL,
                    nonce BLOB NOT NULL,
                    algorithm TEXT NOT NULL,
                    key_id TEXT NOT NULL,
                    updated_at_ns INTEGER NOT NULL,
                    PRIMARY KEY(endpoint_id, field_path)
                );
                INSERT INTO meta(key,value) VALUES('schema_version','2');
                INSERT INTO schema_migrations(version,name,checksum,applied_at_ns)
                    VALUES(1,'001_initial','x',1),(2,'002_management_control','y',2);
                INSERT INTO devices(id,name) VALUES('d1','D1');
                INSERT INTO devices(id,name,profile) VALUES('d2','D2','s7-1200');
                INSERT INTO endpoints(id,device_id,driver_id,connection_json,desired_running,updated_at_ns)
                    VALUES('e1','d1','simulator','{}',1,7);
                INSERT INTO tasks(endpoint_id,id,mode,interval_ms,binding_kind,binding_config_json)
                    VALUES('e1','t1','poll',100,'simulator.points','{}');
                "#,
            )
            .unwrap();
            conn.execute(
                "INSERT INTO endpoint_secrets(endpoint_id,field_path,ciphertext,nonce,algorithm,key_id,updated_at_ns)
                 VALUES('e1','password',?1,?2,'aead','k1',8)",
                params![vec![1u8, 2, 3], vec![9u8]],
            )
            .unwrap();
        }
        // 前置确认：确实是 v2
        {
            let conn = Connection::open(&path).unwrap();
            let ver: String = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(ver, "2");
        }
        // 升级（自锁会在此永久 hanging，CI 超时即失败）
        let s = ConfigStore::open(&path).unwrap();
        {
            let conn = s.conn.lock().unwrap();
            let ver: String = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(ver, "7");
            // 004：profile 列已删除，但设备行本身保留（仅去列，不丢行）
            let cols: Vec<String> = conn
                .prepare("PRAGMA table_info(devices)")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(cols, vec!["id".to_string(), "name".to_string()]);
            let d2: String = conn
                .query_row("SELECT name FROM devices WHERE id='d2'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(d2, "D2");
            // 旧业务行全部还在
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE endpoint_id='e1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
            let blob: Vec<u8> = conn
                .query_row(
                    "SELECT ciphertext FROM endpoint_secrets WHERE endpoint_id='e1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(blob, vec![1u8, 2, 3]);
        }
        // 公共 API 视角：endpoint/任务可读，event_tasks 可用
        assert_eq!(s.list_tasks("e1").unwrap().len(), 1);
        assert!(s.list_event_tasks("e1").unwrap().is_empty());
        s.replace_event_tasks("e1", &[event_task("al")]).unwrap();
        assert_eq!(s.list_event_tasks("e1").unwrap().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn v4_file_db_upgrades_to_v5_endpoint_name_without_data_loss() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mesa-config-v4up-{}-{}.db",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                PRAGMA journal_mode=WAL;
                CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT NOT NULL);
                CREATE TABLE endpoints(
                    id TEXT PRIMARY KEY,
                    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
                    driver_id TEXT NOT NULL,
                    connection_json TEXT NOT NULL,
                    desired_running INTEGER NOT NULL DEFAULT 0,
                    updated_at_ns INTEGER NOT NULL
                );
                CREATE TABLE schema_migrations(
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    applied_at_ns INTEGER NOT NULL
                );
                INSERT INTO meta(key,value) VALUES('schema_version','4');
                INSERT INTO schema_migrations(version,name,checksum,applied_at_ns)
                    VALUES(1,'001_initial','x',1),(2,'002_management_control','y',2),
                    (3,'003_event_tasks','z',3),(4,'004_remove_device_profile','w',4);
                INSERT INTO devices(id,name) VALUES('d1','D1');
                INSERT INTO endpoints(id,device_id,driver_id,connection_json,desired_running,updated_at_ns)
                    VALUES('e1','d1','simulator','{}',1,7);
                "#,
            )
            .unwrap();
        }
        let s = ConfigStore::open(&path).unwrap();
        {
            let conn = s.conn.lock().unwrap();
            let ver: String = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(ver, "7");
            // 旧行以 id 回填 name，业务行保留
            let old_name: String = conn
                .query_row("SELECT name FROM endpoints WHERE id='e1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(old_name, "e1");
        }
        // 公共 API：旧行可读且名已回填；新写入必须带非空名，空名拒绝
        let got = s.get_endpoint("e1").unwrap().unwrap();
        assert_eq!(got.name, "e1");
        let mut named = got.clone();
        named.name = "NCK".into();
        assert!(s.update_endpoint(&named).unwrap());
        assert_eq!(s.get_endpoint("e1").unwrap().unwrap().name, "NCK");
        named.name = "  ".into();
        assert!(s.update_endpoint(&named).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// R1.2 回归：现实 v5 文件库 open() 必须一次走到 v6，且 secret/tasks/
    /// revision 业务行无损，006 幂等表已建。构造方式同 v2/v4 升级测试
    ///（手写 v5 形态 + 业务行，再 ConfigStore::open()）。
    #[test]
    fn v5_file_db_upgrades_to_v6_without_data_loss() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mesa-config-v5up-{}-{}.db",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                PRAGMA journal_mode=WAL;
                CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT NOT NULL);
                CREATE TABLE endpoints(
                    id TEXT PRIMARY KEY,
                    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE RESTRICT,
                    driver_id TEXT NOT NULL,
                    name TEXT NOT NULL DEFAULT '',
                    connection_json TEXT NOT NULL,
                    desired_running INTEGER NOT NULL DEFAULT 0,
                    updated_at_ns INTEGER NOT NULL
                );
                CREATE TABLE tasks(
                    endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
                    id TEXT NOT NULL,
                    mode TEXT NOT NULL,
                    interval_ms INTEGER,
                    binding_kind TEXT NOT NULL,
                    binding_config_json TEXT NOT NULL,
                    PRIMARY KEY(endpoint_id, id)
                );
                CREATE TABLE config_revision(
                    endpoint_id TEXT PRIMARY KEY REFERENCES endpoints(id) ON DELETE CASCADE,
                    revision INTEGER NOT NULL
                );
                CREATE TABLE schema_migrations(
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    applied_at_ns INTEGER NOT NULL
                );
                CREATE TABLE endpoint_secrets(
                    endpoint_id TEXT NOT NULL,
                    field_path TEXT NOT NULL,
                    ciphertext BLOB NOT NULL,
                    nonce BLOB NOT NULL,
                    algorithm TEXT NOT NULL,
                    key_id TEXT NOT NULL,
                    updated_at_ns INTEGER NOT NULL,
                    PRIMARY KEY(endpoint_id, field_path)
                );
                INSERT INTO meta(key,value) VALUES('schema_version','5');
                INSERT INTO schema_migrations(version,name,checksum,applied_at_ns)
                    VALUES(1,'001_initial','x',1),(2,'002_management_control','y',2),
                    (3,'003_event_tasks','z',3),(4,'004_remove_device_profile','w',4),
                    (5,'005_endpoint_name','v',5);
                INSERT INTO devices(id,name) VALUES('d1','D1');
                INSERT INTO endpoints(id,device_id,driver_id,name,connection_json,desired_running,updated_at_ns)
                    VALUES('e1','d1','simulator','E1','{"a":1}',1,7);
                INSERT INTO tasks(endpoint_id,id,mode,interval_ms,binding_kind,binding_config_json)
                    VALUES('e1','t1','poll',100,'simulator.points','{}');
                INSERT INTO config_revision(endpoint_id,revision) VALUES('e1',2);
                INSERT INTO endpoint_secrets(endpoint_id,field_path,ciphertext,nonce,algorithm,key_id,updated_at_ns)
                    VALUES('e1','password',X'0102',X'09','aead','k1',8);
                "#,
            )
            .unwrap();
        }
        let s = ConfigStore::open(&path).unwrap();
        {
            let conn = s.conn.lock().unwrap();
            let ver: String = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(ver, "7");
            // 006 幂等表已建
            let tbl: String = conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name='bootstrap_idempotency'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tbl, "bootstrap_idempotency");
            // secret 行无损
            let blob: Vec<u8> = conn
                .query_row(
                    "SELECT ciphertext FROM endpoint_secrets WHERE endpoint_id='e1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(blob, vec![1u8, 2]);
        }
        // 公共 API：业务行全部可读（device/endpoint/tasks/revision）
        assert_eq!(s.get_device("d1").unwrap().unwrap().name, "D1");
        assert_eq!(s.get_endpoint("e1").unwrap().unwrap().name, "E1");
        assert_eq!(s.list_tasks("e1").unwrap().len(), 1);
        assert_eq!(s.current_revision("e1").unwrap(), 2);
        // 升级后幂等读写可用
        assert!(s.bootstrap_idempotency_get("k").unwrap().is_none());
        s.bootstrap_idempotency_put("k", "h", "d1", "e1", "{}")
            .unwrap();
        assert!(s.bootstrap_idempotency_get("k").unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    /// P1：source_label 快照语义——Some 覆盖、None 清 NULL；point_id/key 不变。
    #[test]
    fn point_source_label_snapshot_semantics() {
        let s = mem();
        s.create_device(&dev("d1")).unwrap();
        s.create_endpoint(&ep("e1", "d1")).unwrap();
        let defs1 = s
            .assign_point_ids(
                "e1",
                &[
                    desc_label("a", DataType::F64, Some("DB10.DBD20")),
                    desc_label("b", DataType::Bool, None),
                ],
            )
            .unwrap();
        let id_a = defs1.iter().find(|d| d.point_key == "a").unwrap().point_id;
        assert_eq!(
            defs1
                .iter()
                .find(|d| d.point_key == "a")
                .unwrap()
                .source_label
                .as_deref(),
            Some("DB10.DBD20")
        );
        // 改地址：label 更新，id 不变
        let defs2 = s
            .assign_point_ids(
                "e1",
                &[
                    desc_label("a", DataType::F64, Some("DB20.DBD40")),
                    desc_label("b", DataType::Bool, Some("M10.0")),
                ],
            )
            .unwrap();
        assert_eq!(
            defs2.iter().find(|d| d.point_key == "a").unwrap().point_id,
            id_a
        );
        assert_eq!(
            defs2
                .iter()
                .find(|d| d.point_key == "a")
                .unwrap()
                .source_label
                .as_deref(),
            Some("DB20.DBD40")
        );
        // Driver 回 None：必须清 NULL，不得保留旧值
        let defs3 = s
            .assign_point_ids("e1", &[desc_label("a", DataType::F64, None)])
            .unwrap();
        assert_eq!(
            defs3.iter().find(|d| d.point_key == "a").unwrap().point_id,
            id_a
        );
        assert_eq!(
            defs3
                .iter()
                .find(|d| d.point_key == "a")
                .unwrap()
                .source_label,
            None
        );
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
        let b_entry = all.iter().find(|(k, _, _, _, _)| k == "b").unwrap();
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
        // P0-1 回归：内存库 Secret 全程不碰磁盘，CWD 下不得出现 key 文件
        //（旧实现会在 crate 目录生成 data/master.key 并被误提交）。
        assert!(
            !std::path::Path::new("data/master.key").exists(),
            "in-memory secret must not create data/master.key under CWD"
        );
        assert!(
            !std::path::Path::new("./master.key").exists(),
            "in-memory secret must not create ./master.key under CWD"
        );
    }

    /// P0-1 Final：master key 是 per-store 语义，不是进程全局。
    /// 同一测试进程内：in-memory（固定测试 key）先访问 Secret，再开两个
    /// 不同目录的文件库各自 put/get，drop 重开后三者必须各自可解密。
    /// 旧全局 `MASTER_KEY_CACHE` 下文件库会命中 `[0xA5;32]`、key 文件甚至
    /// 不生成，重启即失密——本测试逐项断言锁死。
    #[test]
    fn master_key_is_per_store_not_process_global() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "mesa-keyscope-{}-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));

        // 1) in-memory 先访问 Secret（旧全局缓存会被固定测试 key 污染）
        let m = mem();
        m.create_device(&dev("d0")).unwrap();
        m.create_endpoint(&ep("e0", "d0")).unwrap();
        m.put_secret("e0", "password", "mem-secret", "k1").unwrap();
        assert_eq!(
            m.get_secret("e0", "password").unwrap().as_deref(),
            Some("mem-secret")
        );

        // 2) 文件库 A：key 文件必须真实生成（旧实现直接命中缓存，不生成）
        let a_db = base.join("a").join("mesa.db");
        let a = ConfigStore::open(&a_db).unwrap();
        a.create_device(&dev("da")).unwrap();
        a.create_endpoint(&ep("ea", "da")).unwrap();
        a.put_secret("ea", "password", "a-secret", "k1").unwrap();
        let a_key = base.join("a").join("master.key");
        assert!(
            a_key.is_file(),
            "file store must generate its own master.key"
        );
        drop(a);

        // 3) 文件库 B：另一独立 key
        let b_db = base.join("b").join("mesa.db");
        let b = ConfigStore::open(&b_db).unwrap();
        b.create_device(&dev("db")).unwrap();
        b.create_endpoint(&ep("eb", "db")).unwrap();
        b.put_secret("eb", "password", "b-secret", "k1").unwrap();
        drop(b);
        assert_ne!(
            std::fs::read(base.join("a").join("master.key")).unwrap(),
            std::fs::read(base.join("b").join("master.key")).unwrap(),
            "distinct DB dirs must hold distinct keys"
        );

        // 4) 全部重开：各自可解密（旧实现 A/B 会读到 [0xA5;32] 而解密失败）
        let a2 = ConfigStore::open(&a_db).unwrap();
        assert_eq!(
            a2.get_secret("ea", "password").unwrap().as_deref(),
            Some("a-secret")
        );
        let b2 = ConfigStore::open(&b_db).unwrap();
        assert_eq!(
            b2.get_secret("eb", "password").unwrap().as_deref(),
            Some("b-secret")
        );
        // in-memory 不受文件库影响
        assert_eq!(
            m.get_secret("e0", "password").unwrap().as_deref(),
            Some("mem-secret")
        );

        std::fs::remove_dir_all(&base).ok();
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
    fn migration_chain_intact_through_003() {
        let s = mem();
        let conn = s.conn.lock().unwrap();
        let ver: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // PR7 起 SCHEMA_VERSION=3，DeviceProfile 删除后升至 4，Endpoint.name 后升至 5；
        // 002/003 本身仍必须存在且已应用（增量链不断）
        assert!(ver == "7", "新库应为 v7，got {ver}");
        let has2: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=2)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(has2, "002 迁移记录不得丢失");
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert!(cnt >= 4, "至少 4 条迁移");
        // P1：007 记录存在且 point_registry 含 source_label 列
        let has7: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=7)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(has7, "007 迁移记录不得丢失");
        let has_col: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('point_registry') WHERE name='source_label')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(has_col, "point_registry 必须含 source_label 列");
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

    // ---- M5 device-bootstrap ----
    //
    // 中文注释（仓库规范）：原子事务 + 幂等键的单测，验证“要么全落库要么全回滚”。

    fn bootstrap_ep(id: &str, device: &str) -> EndpointRecord {
        EndpointRecord {
            id: id.into(),
            name: format!("{id} 名称"),
            device_id: device.into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: true,
            updated_at_ns: 0,
        }
    }

    #[test]
    fn bootstrap_tx_all_or_nothing() {
        let s = mem();
        let rev = s
            .bootstrap_device_tx_with_plaintext(
                &dev("d1"),
                &bootstrap_ep("e1", "d1"),
                &[],
                &[task("t1", 1000)],
            )
            .unwrap();
        assert_eq!(rev, 1);
        assert!(s.get_device("d1").unwrap().is_some());
        assert!(s.get_endpoint("e1").unwrap().is_some());
        assert_eq!(s.list_tasks("e1").unwrap().len(), 1);
    }

    #[test]
    fn bootstrap_tx_duplicate_device_rolls_back_endpoint() {
        let s = mem();
        // 先建 d1（占住 device id）
        s.create_device(&dev("d1")).unwrap();
        // bootstrap 同 device id 必须 Duplicate，且 endpoint 不得半落库
        let r = s.bootstrap_device_tx_with_plaintext(
            &dev("d1"),
            &bootstrap_ep("e2", "d1"),
            &[],
            &[task("t1", 1000)],
        );
        assert!(matches!(r, Err(StoreError::Duplicate(_))));
        assert!(s.get_endpoint("e2").unwrap().is_none());
        assert!(s.list_tasks("e2").unwrap().is_empty());
    }

    #[test]
    fn bootstrap_tx_bad_tasks_rolls_back_all() {
        let s = mem();
        // 非法 task（空 id）→ Validation，device/endpoint 同样不得残留
        let mut bad = task("", 1000);
        bad.id = "".into();
        let r = s.bootstrap_device_tx_with_plaintext(
            &dev("d9"),
            &bootstrap_ep("e9", "d9"),
            &[],
            &[bad],
        );
        assert!(matches!(r, Err(StoreError::Validation(_))));
        assert!(s.get_device("d9").unwrap().is_none());
        assert!(s.get_endpoint("e9").unwrap().is_none());
    }

    #[test]
    fn bootstrap_idempotency_put_get_roundtrip() {
        let s = mem();
        assert!(s.bootstrap_idempotency_get("k1").unwrap().is_none());
        s.bootstrap_idempotency_put("k1", "hash-a", "d1", "e1", r#"{"ok":true}"#)
            .unwrap();
        let row = s.bootstrap_idempotency_get("k1").unwrap().unwrap();
        assert_eq!(row.0, "hash-a");
        assert_eq!(row.1, r#"{"ok":true}"#);
    }

    #[test]
    fn bootstrap_compensate_removes_endpoint_then_device() {
        let s = mem();
        s.bootstrap_device_tx_with_plaintext(
            &dev("d1"),
            &bootstrap_ep("e1", "d1"),
            &[],
            &[task("t1", 1000)],
        )
        .unwrap();
        s.bootstrap_compensate("d1", "e1").unwrap();
        assert!(s.get_endpoint("e1").unwrap().is_none());
        assert!(s.get_device("d1").unwrap().is_none());
        // 幂等删除：重复补偿不报错
        s.bootstrap_compensate("d1", "e1").unwrap();
    }

    /// RC2 修3：按 device 清理幂等记录（delete_device 第二层防御）。
    #[test]
    fn bootstrap_idempotency_delete_by_device() {
        let s = mem();
        s.bootstrap_idempotency_put("k1", "h", "d1", "e1", "{}")
            .unwrap();
        s.bootstrap_idempotency_put("k2", "h", "d1", "e2", "{}")
            .unwrap();
        s.bootstrap_idempotency_put("k3", "h", "d9", "e9", "{}")
            .unwrap();
        assert_eq!(s.bootstrap_idempotency_delete_by_device("d1").unwrap(), 2);
        assert!(s.bootstrap_idempotency_get("k1").unwrap().is_none());
        assert!(s.bootstrap_idempotency_get("k2").unwrap().is_none());
        // 它设备记录保留
        assert!(s.bootstrap_idempotency_get("k3").unwrap().is_some());
        // 不存在 device 清理返回 0，不报错
        assert_eq!(
            s.bootstrap_idempotency_delete_by_device("ghost").unwrap(),
            0
        );
    }
}
