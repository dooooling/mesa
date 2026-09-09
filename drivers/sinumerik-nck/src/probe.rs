//! NCK 探测（ADR 0001 §8：S7Comm 会话可达性 + 诚实身份）。
//!
//! V1 能证明的：TCP → COTP → Setup 建连成功（对端说 S7Comm）。
//! V1 不能证明的：对端是 SINUMERIK（family/model 置 None，绝不按端口/TSAP
//! 脑补身份；旧 `sinumerik` 驱动的 `UNCONFIRMED` 教训在此 recast）。
//!
//! 身份确认路径（PR7/真机，全部前置）：
//!
//! 1. 官方变量表选永久只读跨版本稳定 anchor → 填 catalog；
//! 2. anchor 0x82 读成功 → vendor/family 置值 + topology 实例回填；
//! 3. SZL 模块标识作为交叉证据（transport 已有 `szl`，语义待确认）。
//!
//! 在此之前 probe 永远携带 `NCK_ANCHOR_PENDING` warning。

use mesa_core_types::{ProbeReport, ProbeWarning};

use crate::config::NckConnConfig;
use mesa_s7_transport::{S7ConnectOptions, S7Session};

/// anchor 缺失警告码（真机回填后消失；Core 侧 profile 永不据此匹配）。
pub const NCK_ANCHOR_PENDING: &str = "NCK_ANCHOR_PENDING";

/// 建连探测：成功 → reachable + 待确认警告；失败 → 不可达规范报告。
pub async fn probe_with_session(cfg: &NckConnConfig) -> ProbeReport {
    let opts = S7ConnectOptions {
        host: cfg.host.clone(),
        port: cfg.port,
        local_tsap: cfg.local_tsap,
        remote_tsap: cfg.remote_tsap,
        timeout_ms: cfg.timeout_ms,
        requested_pdu_length: cfg.requested_pdu_length,
    };
    let session = match S7Session::connect(opts).await {
        Ok(s) => s,
        Err(e) => {
            return ProbeReport::unreachable(
                e.code.clone(),
                format!("NCK 会话建立失败: {}", e.message),
            );
        }
    };
    let pdu = session.negotiated_pdu_length();
    session.disconnect().await.ok();
    ProbeReport {
        reachable: true,
        vendor: None,
        family: None,
        model: None,
        firmware: None,
        model_confidence: None,
        capabilities: vec![],
        warnings: vec![ProbeWarning {
            code: NCK_ANCHOR_PENDING.into(),
            message: format!(
                "S7Comm 会话已建立（协商 PDU {pdu}），但 SINUMERIK 身份未确认：\
                 anchor 变量待官方变量表 + 真机回填 catalog，在此之前 family/model 恒为 None"
            ),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{NckFixture, NckFixtureState};

    fn cfg_for(port: u16) -> NckConnConfig {
        NckConnConfig::from_json(&serde_json::json!({
            "host": "127.0.0.1", "port": port, "family": "840d-sl",
            "local_tsap": 256, "remote_tsap": 258,
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn probe_reachable_but_identity_unconfirmed() {
        let fx = NckFixture::spawn(NckFixtureState::default()).await;
        let report = probe_with_session(&cfg_for(fx.addr.port())).await;
        assert!(report.reachable);
        assert!(report.vendor.is_none(), "无 anchor 不得断言 vendor");
        assert!(report.family.is_none(), "无 anchor 不得断言 family");
        assert!(report.model_confidence.is_none());
        assert!(
            report.warnings.iter().any(|w| w.code == NCK_ANCHOR_PENDING),
            "必须携带 anchor 缺失警告"
        );
        report.validate().expect("可达报告必须合法");
    }

    #[tokio::test]
    async fn probe_dead_port_is_unreachable() {
        // 未监听端口（连接被拒）→ 不可达规范形态。
        let report = cfg_for(1);
        let report = probe_with_session(&report).await;
        assert!(!report.reachable);
        assert!(report.vendor.is_none());
        assert!(!report.warnings.is_empty(), "不可达必须道明原因");
        report.validate().expect("不可达报告必须合法");
    }
}
