//! SINUMERIK 动态探测（PR11 Checkpoint A）。
//!
//! 只允许经公共 [`mesa_opcua_transport::OpcUaTransport`] 调用，禁止直连
//! `async-opcua-client`（本文件无任何 async-opcua 导入）；只读事实采集，不建订阅、不写。
//!
//! 流程：建连 → NamespaceArray → 标准 BuildInfo 身份 → Objects 浅浏览 → 断开。
//!
//! 身份诚实原则（Review Gate：Probe 不过度推断）：
//! - `vendor/model/firmware` 原样透传 BuildInfo 事实，不改写；
//! - `family = "SINUMERIK"` 仅当 ProductName 含 `sinumerik`（大小写不敏感）——
//!   这是目前唯一被采信的强证据（PR12 真机验证若发现反例，修本规则 + 补回归）；
//! - 仅有 Siemens 厂商痕迹（vendor/namespace 含 `siemens`）但无 `sinumerik` 字样时，
//!   family 保持 None 并报 `SINUMERIK_UNCONFIRMED`（可能是 SIMATIC 等其他 Siemens
//!   设备，绝不猜成 SINUMERIK）；
//! - 全无 Siemens 痕迹时同样 `SINUMERIK_UNCONFIRMED`（按通用 OPC UA 设备处理，
//!   不拒绝采集——拒绝是 Core profile 匹配的事，不是 probe 的事）。

use mesa_core_types::{CapabilityItem, CapabilityState, ProbeReport, ProbeWarning};
use mesa_driver_sdk::SdkDriverError;
use mesa_opcua_transport::{OpcUaTransport, UaBrowseRequest, UaNodeRef, UaTransportError};

/// 标准 Server 身份节点（ns=0，OPC UA 标准节点集）：
/// ManufacturerName=2263 → vendor，ProductName=2261 → model，SoftwareVersion=2264 → firmware。
const IDENT_NODES: [(u16, u32); 3] = [(0, 2263), (0, 2261), (0, 2264)];
/// Objects 根（仅一层浅浏览确认 browse 能力，不翻页聚合）。
const OBJECTS_ROOT: (u16, u32) = (0, 85);

/// 大小写不敏感子串判断（ASCII 场景足够；设备字符串均为 ASCII）。
fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase().contains(needle)
}

/// 从原生 DataValue 取非空 String（单点 BAD/类型不符一律 None，不抛错）。
/// 状态缺席按 Good 处理（与通用 OPC UA 驱动一致：服务端省略 Good 默认值）。
fn dv_string(dv: &mesa_opcua_transport::UaDataValue) -> Option<String> {
    let st = dv.status.unwrap_or(opcua_types::StatusCode::Good);
    if st.is_bad() {
        return None;
    }
    let s = match dv.value.as_ref()? {
        opcua_types::Variant::String(s) => s.as_ref().to_string(),
        opcua_types::Variant::ByteString(bs) => {
            String::from_utf8(bs.value.clone().unwrap_or_default()).ok()?
        }
        _ => return None,
    };
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// 通用探测流程（Native 与 Fake 共用；Fake 用于单测脚本化）。
pub async fn probe_with_transport(
    transport: &dyn OpcUaTransport,
) -> Result<ProbeReport, SdkDriverError> {
    if let Err(e) = transport.connect().await {
        // 建连失败 = 设备不可达（探测结果，不是 Err）
        return Ok(ProbeReport::unreachable("CONNECTION_FAILED", e.to_string()));
    }
    // 单出口不断开——所有返回路径先经 disconnect（best-effort）。
    let report = probe_connected(transport).await;
    let _ = transport.disconnect().await;
    Ok(report)
}

/// 传输错误 → 四态：只认明确语义的 StatusCode，
/// 超时/会话/服务/内部失败一律 Unknown，绝不谎报 AccessDenied。
fn capability_state_from_ua_error(e: &UaTransportError) -> CapabilityState {
    match e.status_code.map(opcua_types::StatusCode::from) {
        Some(opcua_types::StatusCode::BadUserAccessDenied) => CapabilityState::AccessDenied,
        Some(opcua_types::StatusCode::BadNodeIdUnknown)
        | Some(opcua_types::StatusCode::BadNodeIdInvalid) => CapabilityState::NotPresent,
        _ => CapabilityState::Unknown,
    }
}

async fn probe_connected(transport: &dyn OpcUaTransport) -> ProbeReport {
    let mut warnings = Vec::new();

    // 1. NamespaceArray：全局环境事实 + Siemens 证据来源之一。
    let ns_uris: Option<Vec<String>> = match transport.read_namespace_array().await {
        Ok(uris) => Some(uris),
        Err(e) => {
            warnings.push(ProbeWarning {
                code: "NAMESPACE_PARTIAL".into(),
                message: format!("NamespaceArray 读取失败: {e}"),
            });
            None
        }
    };

    // 2. 标准 BuildInfo 身份（逐点 BAD 容忍）。
    let (vendor, model, firmware, buildinfo_read_ok) = match transport
        .read(
            &IDENT_NODES
                .iter()
                .map(|(ns, id)| UaNodeRef::numeric(*ns, *id))
                .collect::<Vec<_>>(),
        )
        .await
    {
        Ok(vals) => {
            let get = |i: usize| vals.get(i).and_then(dv_string);
            let (v, m, f) = (get(0), get(1), get(2));
            if v.is_none() && m.is_none() && f.is_none() {
                warnings.push(ProbeWarning {
                    code: "MODEL_UNDETECTED".into(),
                    message: "标准 BuildInfo 无可用身份信息".into(),
                });
            }
            (v, m, f, true)
        }
        Err(e) => {
            warnings.push(ProbeWarning {
                code: "IDENTITY_UNAVAILABLE".into(),
                message: format!("BuildInfo 读取失败: {e}"),
            });
            (None, None, None, false)
        }
    };

    // 3. SINUMERIK 身份判定（仅事实组合，不猜测）：
    // 强证据 = ProductName 含 sinumerik → family + high；
    // 弱证据 = vendor/namespace 含 siemens 但无 sinumerik 字样 → family None + UNCONFIRMED；
    // 无证据 → family None + UNCONFIRMED。
    let siemens_ns = ns_uris
        .as_ref()
        .is_some_and(|uris| uris.iter().any(|u| contains_ci(u, "siemens")));
    let vendor_siemens = vendor.as_deref().is_some_and(|v| contains_ci(v, "siemens"));
    let model_sinumerik = model
        .as_deref()
        .is_some_and(|m| contains_ci(m, "sinumerik"));
    let (family, model_confidence) = if model_sinumerik {
        (Some("SINUMERIK".to_string()), Some("high".to_string()))
    } else {
        if vendor_siemens || siemens_ns {
            warnings.push(ProbeWarning {
                code: "SINUMERIK_UNCONFIRMED".into(),
                message: "发现 Siemens 痕迹但 ProductName 无 SINUMERIK 字样，不断言 family（可能为其他 Siemens 设备）".into(),
            });
        } else {
            warnings.push(ProbeWarning {
                code: "SINUMERIK_UNCONFIRMED".into(),
                message: "未发现 Siemens/SINUMERIK 特征，按通用 OPC UA 设备处理".into(),
            });
        }
        (None, None)
    };

    // read 证据合并：NamespaceArray 与 BuildInfo 任一 Read 成功即 Available。
    let (read_state, read_detail) = match (&ns_uris, buildinfo_read_ok) {
        (Some(_), _) | (_, true) => (CapabilityState::Available, None),
        (None, false) => {
            // ns_uris 为 None 说明已有 NAMESPACE_PARTIAL；此处不再重复定级细节，
            // 按 Unknown 保守处理（原始错误已在 warning 中）。
            (CapabilityState::Unknown, None)
        }
    };

    // 4. Objects 浅浏览确认 browse 能力（单页即可，不翻页）。
    let (browse_state, browse_detail) = match transport
        .browse(UaBrowseRequest {
            node: UaNodeRef::numeric(OBJECTS_ROOT.0, OBJECTS_ROOT.1),
            max_refs: 100,
        })
        .await
    {
        Ok(_) => (CapabilityState::Available, None),
        Err(e) => (
            capability_state_from_ua_error(&e),
            Some(format!("Objects 浏览失败: {e}")),
        ),
    };

    // subscribe 本次未建订阅，无资格断言——直接省略（缺席≠不支持）。
    let capabilities = vec![
        CapabilityItem {
            id: "read".into(),
            state: read_state,
            detail: read_detail,
        },
        CapabilityItem {
            id: "browse".into(),
            state: browse_state,
            detail: browse_detail,
        },
    ];
    ProbeReport {
        reachable: true,
        vendor,
        family,
        model,
        firmware,
        model_confidence,
        capabilities,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_opcua_transport::{FakeOpcUaTransport, fake_browse_node};
    use opcua_types::{DataValue, StatusCode};

    const SIEMENS_NS: &str = "http://www.siemens.com/sinumerik";
    const STD_NS: &str = "http://opcfoundation.org/UA/";

    /// SINUMERIK 形态 fixture（PR11 的设备语义假设，PR12 真机验证后冻结）：
    /// Siemens 命名空间 + BuildInfo 报 SINUMERIK + Objects 可浏览。
    fn sinumerik_fake() -> FakeOpcUaTransport {
        FakeOpcUaTransport::new()
            .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()])
            .with_read(&UaNodeRef::numeric(0, 2263), DataValue::new_now("Siemens"))
            .with_read(
                &UaNodeRef::numeric(0, 2261),
                DataValue::new_now("SINUMERIK 840D sl"),
            )
            .with_read(&UaNodeRef::numeric(0, 2264), DataValue::new_now("V5.24"))
            .with_browse_pages(
                &UaNodeRef::numeric(0, 85),
                vec![mesa_opcua_transport::UaBrowsePage {
                    nodes: vec![fake_browse_node(UaNodeRef::numeric(0, 2253), "Server")],
                    continuation_point: None,
                }],
            )
    }

    fn cap<'a>(r: &'a ProbeReport, id: &str) -> &'a CapabilityItem {
        r.capabilities
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("缺少 capability {id}"))
    }

    #[tokio::test]
    async fn sinumerik_identity_confirmed_high() {
        let r = probe_with_transport(&sinumerik_fake()).await.expect("Ok");
        assert!(r.reachable);
        assert_eq!(r.vendor.as_deref(), Some("Siemens"));
        assert_eq!(r.family.as_deref(), Some("SINUMERIK"));
        assert_eq!(r.model.as_deref(), Some("SINUMERIK 840D sl"));
        assert_eq!(r.firmware.as_deref(), Some("V5.24"));
        assert_eq!(r.model_confidence.as_deref(), Some("high"));
        assert!(
            r.warnings.iter().all(|w| w.code != "SINUMERIK_UNCONFIRMED"),
            "强证据下不应有 UNCONFIRMED"
        );
        assert_eq!(cap(&r, "read").state, CapabilityState::Available);
        assert_eq!(cap(&r, "browse").state, CapabilityState::Available);
        assert!(r.capabilities.iter().all(|c| c.id != "subscribe"));
    }

    #[tokio::test]
    async fn siemens_without_sinumerik_token_does_not_claim_family() {
        // SIMATIC 形态：vendor Siemens，但 ProductName 无 SINUMERIK → family 必须 None。
        let t = FakeOpcUaTransport::new()
            .with_namespace_array(vec![
                STD_NS.to_string(),
                "http://www.siemens.com/simatic".to_string(),
            ])
            .with_read(&UaNodeRef::numeric(0, 2263), DataValue::new_now("Siemens"))
            .with_read(
                &UaNodeRef::numeric(0, 2261),
                DataValue::new_now("SIMATIC S7-1500"),
            )
            .with_read(&UaNodeRef::numeric(0, 2264), DataValue::new_now("V3.1"));
        let r = probe_with_transport(&t).await.expect("Ok");
        assert!(r.reachable);
        assert!(r.family.is_none(), "绝不能把 SIMATIC 猜成 SINUMERIK");
        assert!(r.model_confidence.is_none());
        assert_eq!(r.model.as_deref(), Some("SIMATIC S7-1500"));
        assert!(r.warnings.iter().any(|w| w.code == "SINUMERIK_UNCONFIRMED"));
    }

    #[tokio::test]
    async fn generic_device_stays_unconfirmed() {
        let t = FakeOpcUaTransport::new()
            .with_namespace_array(vec![STD_NS.to_string(), "urn:test:device".to_string()])
            .with_read(
                &UaNodeRef::numeric(0, 2263),
                DataValue::new_now("TestVendor"),
            )
            .with_read(
                &UaNodeRef::numeric(0, 2261),
                DataValue::new_now("TestModel"),
            );
        let r = probe_with_transport(&t).await.expect("Ok");
        assert!(r.reachable);
        assert!(r.family.is_none());
        assert!(r.model_confidence.is_none());
        assert!(r.warnings.iter().any(|w| w.code == "SINUMERIK_UNCONFIRMED"));
    }

    #[tokio::test]
    async fn missing_identity_warns_model_undetected() {
        let t = FakeOpcUaTransport::new()
            .with_create_status(&UaNodeRef::numeric(0, 2261), StatusCode::BadNodeIdUnknown);
        let r = probe_with_transport(&t).await.expect("Ok");
        assert!(r.reachable);
        assert!(r.model.is_none());
        assert!(r.family.is_none());
        assert!(r.warnings.iter().any(|w| w.code == "MODEL_UNDETECTED"));
    }

    #[tokio::test]
    async fn connection_probe_refused_port_is_unreachable() {
        let d = crate::SinumerikDriver;
        let mut conn = mesa_driver_sdk::Driver::open_connection(
            &d,
            "t",
            r#"{"endpoint_url":"opc.tcp://127.0.0.1:9","timeout_ms":1000}"#,
        )
        .await
        .expect("open 只解析配置");
        let r = mesa_driver_sdk::DriverConnection::probe(&mut *conn)
            .await
            .expect("不可达是探测结果");
        assert!(!r.reachable);
        assert!(r.warnings.iter().any(|w| w.code == "CONNECTION_FAILED"));
        assert!(r.capabilities.is_empty());
    }

    #[tokio::test]
    async fn open_rejects_bad_config() {
        let d = crate::SinumerikDriver;
        let err = match mesa_driver_sdk::Driver::open_connection(&d, "t", "not-json").await {
            Ok(_) => panic!("非法配置必须拒绝"),
            Err(e) => e,
        };
        assert_eq!(err.code, "BAD_CONFIG");
    }
}
