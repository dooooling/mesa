//! OPC UA 证书管理（方案 §19.3 §8）。
//!
//! 目录约定（与 async-opcua pki_dir 兼容）：
//! ```text
//! data/certificates/opcua/
//!   own/        — 本机 Application Instance Certificate + private key
//!   trusted/    — 受信任的 server 证书（PEM/DER）
//!   issuers/    — 中间 CA（预留）
//!   rejected/   — 首次连接被拒的 server 证书，需手动 trust
//! ```
//! V1 最小能力：首启生成 own 证书、导入/删除 trusted、查看/信任 rejected、到期诊断、
//! SecurityPolicy/MessageSecurityMode 配置透传（禁止默认忽略校验）。

use std::path::{Path, PathBuf};
use std::fs;

use serde::{Deserialize, Serialize};
use sha1::{Sha1, Digest};
use x509_parser::prelude::*;

const CERT_DIR: &str = "data/certificates/opcua";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertInfo {
    /// SHA1 thumbprint（大写十六进制，无冒号）
    pub thumbprint: String,
    /// 文件名
    pub filename: String,
    /// PEM 文本（截断显示，前 100 字符）
    pub pem_preview: String,
    /// Subject DN
    pub subject: String,
    /// Issuer DN
    pub issuer: String,
    /// NotBefore / NotAfter（RFC3339）
    pub not_before: String,
    pub not_after: String,
    /// 是否过期
    pub expired: bool,
    /// 剩余天数（负数表示已过期）
    pub days_remaining: i64,
}

#[derive(Debug, Clone)]
pub struct CertStore {
    base: PathBuf,
}

impl CertStore {
    pub fn new<P: AsRef<Path>>(base: P) -> Self {
        Self { base: base.as_ref().to_path_buf() }
    }

    pub fn default_path() -> PathBuf {
        PathBuf::from(CERT_DIR)
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for sub in &["own", "trusted", "issuers", "rejected", "private"] {
            let p = self.base.join(sub);
            fs::create_dir_all(&p)?;
        }
        Ok(())
    }

    fn store_path(&self, store: &str) -> PathBuf {
        self.base.join(store)
    }

    /// 列出指定 store 的证书
    pub fn list(&self, store: &str) -> Result<Vec<CertInfo>, String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let dir = self.store_path(store);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_file() { continue; }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "pem" && ext != "der" && ext != "crt" { continue; }
            match Self::parse_file(&path) {
                Ok(info) => out.push(info),
                Err(e) => {
                    tracing::warn!(file=%path.display(), error=%e, "证书解析失败，跳过");
                    continue;
                }
            }
        }
        Ok(out)
    }

    fn parse_file(path: &Path) -> Result<CertInfo, String> {
        let data = fs::read(path).map_err(|e| e.to_string())?;
        // 尝试 PEM 解码，若失败则按 DER 处理
        let der = if data.starts_with(b"-----BEGIN") {
            let pem = ::pem::parse(&data).map_err(|e| e.to_string())?;
            pem.contents().to_vec()
        } else {
            data.clone()
        };
        // thumbprint = SHA1(DER)
        let mut hasher = Sha1::new();
        hasher.update(&der);
        let thumbprint = format!("{:X}", hasher.finalize());

        // 解析 X509
        let (_, cert) = X509Certificate::from_der(&der).map_err(|e| format!("x509 解析失败: {e:?}"))?;
        let subject = cert.subject.to_string();
        let issuer = cert.issuer.to_string();
        let not_before = cert.validity.not_before.to_string();
        let not_after = cert.validity.not_after.to_string();
        // 简单过期判断：not_after < now
        let now = chrono::Utc::now();
        let not_after_dt = Self::parse_x509_time(&cert.validity.not_after);
        let not_before_dt = Self::parse_x509_time(&cert.validity.not_before);
        let expired = not_after_dt.map(|t| t < now).unwrap_or(false);
        let days_remaining = not_after_dt.map(|t| (t - now).num_days()).unwrap_or(0);

        let pem_preview = String::from_utf8_lossy(&data).chars().take(100).collect::<String>();
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();

        Ok(CertInfo {
            thumbprint,
            filename,
            pem_preview,
            subject,
            issuer,
            not_before: not_before_dt.map(|t| t.to_rfc3339()).unwrap_or(not_before),
            not_after: not_after_dt.map(|t| t.to_rfc3339()).unwrap_or(not_after),
            expired,
            days_remaining,
        })
    }

    fn parse_x509_time(t: &x509_parser::time::ASN1Time) -> Option<chrono::DateTime<chrono::Utc>> {
        // ASN1Time 转 chrono：尝试解析字符串
        let s = t.to_string();
        // 尝试 RFC2822 或自定义
        chrono::DateTime::parse_from_rfc2822(&s).ok().map(|dt| dt.with_timezone(&chrono::Utc))
            .or_else(|| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S UTC").ok().map(|n| n.and_utc()))
            .or_else(|| chrono::DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&chrono::Utc)))
    }

    /// 导入受信任证书（PEM 文本）
    pub fn add_trusted(&self, pem_text: &str) -> Result<String, String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let pem_text = pem_text.trim();
        if pem_text.is_empty() {
            return Err("PEM 不能为空".into());
        }
        // 校验 PEM
        let pem = ::pem::parse(pem_text).map_err(|e| format!("PEM 解析失败: {e}"))?;
        if pem.tag() != "CERTIFICATE" {
            return Err(format!("PEM tag 需为 CERTIFICATE，实际 {}", pem.tag()));
        }
        let der = pem.contents();
        // thumbprint
        let mut hasher = Sha1::new();
        hasher.update(der);
        let thumbprint = format!("{:X}", hasher.finalize());
        // 去重：已存在则直接返回
        let trusted_dir = self.store_path("trusted");
        let rejected_dir = self.store_path("rejected");
        for dir in [&trusted_dir, &rejected_dir] {
            if let Ok(entries) = fs::read_dir(dir) {
                for e in entries.flatten() {
                    if e.file_name().to_string_lossy().contains(&thumbprint) {
                        return Ok(thumbprint);
                    }
                }
            }
        }
        let filename = format!("{}.pem", thumbprint);
        let path = trusted_dir.join(&filename);
        fs::write(&path, pem_text).map_err(|e| e.to_string())?;
        tracing::info!(thumbprint=%thumbprint, file=%path.display(), "已导入受信任证书");
        Ok(thumbprint)
    }

    /// 删除受信任证书
    pub fn remove_trusted(&self, thumbprint: &str) -> Result<bool, String> {
        let dir = self.store_path("trusted");
        if !dir.exists() { return Ok(false); }
        let tp_upper = thumbprint.to_uppercase();
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let fname = entry.file_name().to_string_lossy().to_uppercase();
            if fname.contains(&tp_upper) {
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                tracing::info!(thumbprint=%thumbprint, "已删除受信任证书");
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 将 rejected 证书移至 trusted
    pub fn trust_rejected(&self, thumbprint: &str) -> Result<bool, String> {
        let rejected_dir = self.store_path("rejected");
        let trusted_dir = self.store_path("trusted");
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let tp_upper = thumbprint.to_uppercase();
        for entry in fs::read_dir(&rejected_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let fname = entry.file_name().to_string_lossy().to_uppercase();
            if fname.contains(&tp_upper) {
                let data = fs::read(entry.path()).map_err(|e| e.to_string())?;
                let dest = trusted_dir.join(entry.file_name());
                fs::write(&dest, &data).map_err(|e| e.to_string())?;
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                tracing::info!(thumbprint=%thumbprint, "已信任 rejected 证书");
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 生成自签名 own 证书（若已存在则跳过），同时兼容 async-opcua 的 pki 布局
    pub fn ensure_own_cert(&self) -> Result<bool, String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let own_dir = self.store_path("own");
        let cert_path = own_dir.join("own.der");
        let key_path = own_dir.join("own.key");
        // 兼容 async-opcua 期望的路径：pki/own/cert.der + pki/private/private.pem
        let alt_cert_path = own_dir.join("cert.der");
        let private_dir = self.base.join("private");
        let alt_key_path = private_dir.join("private.pem");
        if cert_path.exists() && key_path.exists() && alt_cert_path.exists() && alt_key_path.exists() {
            return Ok(false);
        }
        // 使用 rcgen 生成（0.13 API）
        let certified = rcgen::generate_simple_self_signed(vec!["Mesa-opcua".to_string()]).map_err(|e| e.to_string())?;
        let pem = certified.cert.pem();
        let key_pem = certified.key_pair.serialize_pem();
        let der = certified.cert.der().to_vec();
        fs::write(&cert_path, &der).map_err(|e| e.to_string())?;
        fs::write(own_dir.join("own.pem"), pem.clone()).map_err(|e| e.to_string())?;
        fs::write(&key_path, &key_pem).map_err(|e| e.to_string())?;
        // 双写兼容路径
        fs::write(&alt_cert_path, &der).map_err(|e| e.to_string())?;
        let _ = fs::create_dir_all(&private_dir);
        fs::write(&alt_key_path, &key_pem).map_err(|e| e.to_string())?;
        // 限制私钥权限（Unix）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600));
        }
        tracing::info!(cert=%cert_path.display(), "已生成自签名 own 证书");
        Ok(true)
    }

    /// 诊断：返回所有证书的过期情况
    pub fn diagnostics(&self) -> serde_json::Value {
        let mut diag = serde_json::json!({});
        for store in &["own", "trusted", "issuers", "rejected"] {
            let list = self.list(store).unwrap_or_default();
            let expired_count = list.iter().filter(|c| c.expired).count();
            diag[store] = serde_json::json!({
                "count": list.len(),
                "expired": expired_count,
                "certs": list,
            });
        }
        diag
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn cert_store_list_empty() {
        let tmp = TempDir::new().unwrap();
        let store = CertStore::new(tmp.path());
        let list = store.list("trusted").unwrap();
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn ensure_own_generates() {
        let tmp = TempDir::new().unwrap();
        let store = CertStore::new(tmp.path());
        let created = store.ensure_own_cert().unwrap();
        assert!(created);
        let created2 = store.ensure_own_cert().unwrap();
        assert!(!created2);
        let list = store.list("own").unwrap();
        assert!(!list.is_empty());
    }

    #[test]
    fn add_and_remove_trusted() {
        let tmp = TempDir::new().unwrap();
        let store = CertStore::new(tmp.path());
        store.ensure_own_cert().unwrap();
        let own_list = store.list("own").unwrap();
        let pem_path = tmp.path().join("own/own.pem");
        let pem_text = std::fs::read_to_string(&pem_path).unwrap();
        let tp = store.add_trusted(&pem_text).unwrap();
        assert!(!tp.is_empty());
        let trusted = store.list("trusted").unwrap();
        assert_eq!(trusted.len(), 1);
        assert_eq!(trusted[0].thumbprint, tp);
        let removed = store.remove_trusted(&tp).unwrap();
        assert!(removed);
        let trusted2 = store.list("trusted").unwrap();
        assert_eq!(trusted2.len(), 0);
    }
}
