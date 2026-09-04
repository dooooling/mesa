//! OPC UA 动态探测（V1.2.1 §8，feat/dynamic-probe 阶段 6）。
//!
//! 只允许经公共 [`mesa_opcua_transport::OpcUaTransport`] 调用，禁止直连
//! `async-opcua-client`（Stage 2 冻结边界；本文件无任何 async-opcua 导入）。
//! 探测内容：建连 → NamespaceArray → 标准 BuildInfo 身份 → Objects 浅浏览 → 断开。
//! 不建订阅（subscribe 本次不确认 → None），不写任何东西。

use mesa_core_types::{CapabilityItem, CapabilityState, ProbeReport, ProbeWarning};
use mesa_driver_sdk::SdkDriverError;
use mesa_opcua_transport::{OpcUaTransport, UaBrowseRequest, UaNodeRef, UaTransportError};

/// 标准 Server 身份节点（ns=0，OPC UA 标准节点集）：
/// ManufacturerName=2263 → vendor，ProductName=2261 → model，
/// SoftwareVersion=2264 → firmware。
const IDENT_NODES: [(u16, u32); 3] = [(0, 2263), (0, 2261), (0, 2264)];
/// Objects 根（仅做一层浅浏览确认 browse 能力，不翻页聚合）。
const OBJECTS_ROOT: (u16, u32) = (0, 85);

/// 从原生 DataValue 取非空 String（单点 BAD/类型不符一律 None，不抛错）。
/// 状态缺席按 Good 处理（与 P0-A decode_data_value 一致：服务端省略 Good 默认值）。
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
/// 参数为 `&dyn`（连接持有 `Arc<dyn OpcUaTransport>`，与采集共享同一 Arc；
/// async_trait 下泛型 `?Sized` 不可用，故直接用 trait 对象）。
pub async fn probe_with_transport(
    transport: &dyn OpcUaTransport,
) -> Result<ProbeReport, SdkDriverError> {
    if let Err(e) = transport.connect().await {
        // 建连失败 = 设备不可达（探测结果，不是 Err）
        return Ok(ProbeReport::unreachable("CONNECTION_FAILED", e.to_string()));
    }
    // 注意：单出口不断开——所有返回路径先经 disconnect（best-effort）。
    let report = probe_connected(transport).await;
    let _ = transport.disconnect().await;
    Ok(report)
}

/// 传输错误 → 四态（P1-1）：只认明确语义的 StatusCode，
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

    // 1. NamespaceArray：全局环境事实。失败时 capability 取错误映射状态
    //（通常 Unknown），并追加 NAMESPACE_PARTIAL（影响身份解析的全局事项）。
    let (read_state, read_detail) = match transport.read_namespace_array().await {
        Ok(_) => (CapabilityState::Available, None),
        Err(e) => {
            warnings.push(ProbeWarning {
                code: "NAMESPACE_PARTIAL".into(),
                message: format!("NamespaceArray 读取失败: {e}"),
            });
            (
                capability_state_from_ua_error(&e),
                Some(format!("NamespaceArray 读取失败: {e}")),
            )
        }
    };

    // 2. 标准 BuildInfo 身份（逐点 BAD 容忍：单点缺失只影响对应字段）。
    // 同一缺失只报一次：整包失败报 IDENTITY_UNAVAILABLE；读成功但无身份
    // 报 MODEL_UNDETECTED（P0-1 不重复规则）。
    let (vendor, model, firmware) = match transport
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
            (v, m, f)
        }
        Err(e) => {
            warnings.push(ProbeWarning {
                code: "IDENTITY_UNAVAILABLE".into(),
                message: format!("BuildInfo 读取失败: {e}"),
            });
            (None, None, None)
        }
    };

    // 3. Objects 浅浏览确认 browse 能力（单页即可，不翻页）。
    // 失败原因只写 capability detail，不另起 warning（P1-1 不重复规则，
    // BROWSE_CHECK_FAILED 已删除）。
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

    // 能力语义：read/browse 为本次实测结论；subscribe 本次未建订阅，
    // 无资格断言——直接省略该条目（缺席≠不支持）。
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
        family: None,
        model,
        firmware,
        model_confidence: None,
        capabilities,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_opcua_transport::{FakeOpcUaTransport, fake_browse_node};
    use opcua_types::{DataValue, StatusCode};

    fn ident_fake() -> FakeOpcUaTransport {
        FakeOpcUaTransport::new()
            .with_namespace_array(vec![
                "http://opcfoundation.org/UA/".to_string(),
                "urn:test:device".to_string(),
            ])
            .with_read(
                &UaNodeRef::numeric(0, 2263),
                DataValue::new_now("TestVendor"),
            )
            .with_read(
                &UaNodeRef::numeric(0, 2261),
                DataValue::new_now("TestModel"),
            )
            .with_read(&UaNodeRef::numeric(0, 2264), DataValue::new_now("9.9"))
            .with_browse_pages(
                &UaNodeRef::numeric(0, 85),
                vec![mesa_opcua_transport::UaBrowsePage {
                    nodes: vec![fake_browse_node(UaNodeRef::numeric(0, 2253), "Server")],
                    continuation_point: None,
                }],
            )
    }

    fn cap<'a>(
        r: &'a mesa_core_types::ProbeReport,
        id: &str,
    ) -> &'a mesa_core_types::CapabilityItem {
        r.capabilities
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("缺少 capability {id}"))
    }

    #[tokio::test]
    async fn full_identity_probed() {
        let t = ident_fake();
        let r = probe_with_transport(&t).await.expect("Ok");
        assert!(r.reachable);
        assert_eq!(r.vendor.as_deref(), Some("TestVendor"));
        assert_eq!(r.model.as_deref(), Some("TestModel"));
        assert_eq!(r.firmware.as_deref(), Some("9.9"));
        assert!(r.warnings.is_empty());
        assert_eq!(
            cap(&r, "read").state,
            mesa_core_types::CapabilityState::Available
        );
        assert_eq!(
            cap(&r, "browse").state,
            mesa_core_types::CapabilityState::Available
        );
        // subscribe 本次未确认：无条目（缺席≠不支持）
        assert!(r.capabilities.iter().all(|c| c.id != "subscribe"));
    }

    #[tokio::test]
    async fn missing_identity_warns_model_undetected() {
        // 全 BAD 身份 + 空浏览：reachable 照真，MODEL_UNDETECTED 必有
        let t = FakeOpcUaTransport::new()
            .with_create_status(&UaNodeRef::numeric(0, 2261), StatusCode::BadNodeIdUnknown);
        let r = probe_with_transport(&t).await.expect("Ok");
        assert!(r.reachable);
        assert!(r.model.is_none());
        assert!(r.warnings.iter().any(|w| w.code == "MODEL_UNDETECTED"));
        // Fake 默认命名空间为空数组但读取成功
        assert_eq!(
            cap(&r, "read").state,
            mesa_core_types::CapabilityState::Available
        );
    }

    #[tokio::test]
    async fn connection_probe_refused_port_is_unreachable() {
        // 127.0.0.1:9 预期关闭：经已打开的连接探测，建连失败 → Ok(unreachable)。
        // open 自身不建连（lazy），probe 内的 connect 触发失败。
        let d = crate::OpcUaDriver;
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
        // 配置校验在 OpenConnection（probe 复用已开连接）
        let d = crate::OpcUaDriver;
        let err = match mesa_driver_sdk::Driver::open_connection(&d, "t", "not-json").await {
            Ok(_) => panic!("非法配置必须拒绝"),
            Err(e) => e,
        };
        assert_eq!(err.code, "BAD_CONFIG");
    }

    #[test]
    fn ua_error_maps_to_accurate_capability_state() {
        use mesa_opcua_transport::{UaOperation, UaTransportError};
        let err_with = |status: Option<opcua_types::StatusCode>| {
            UaTransportError::service(UaOperation::Read, status, false, "test")
        };
        // 明确语义才判两态
        assert_eq!(
            capability_state_from_ua_error(&err_with(Some(
                opcua_types::StatusCode::BadUserAccessDenied
            ))),
            CapabilityState::AccessDenied
        );
        assert_eq!(
            capability_state_from_ua_error(&err_with(Some(
                opcua_types::StatusCode::BadNodeIdUnknown
            ))),
            CapabilityState::NotPresent
        );
        assert_eq!(
            capability_state_from_ua_error(&err_with(Some(
                opcua_types::StatusCode::BadNodeIdInvalid
            ))),
            CapabilityState::NotPresent
        );
        // 超时/会话/无码一律 Unknown，绝不谎报 AccessDenied
        assert_eq!(
            capability_state_from_ua_error(&err_with(Some(opcua_types::StatusCode::BadTimeout))),
            CapabilityState::Unknown
        );
        assert_eq!(
            capability_state_from_ua_error(&err_with(Some(
                opcua_types::StatusCode::BadSessionClosed
            ))),
            CapabilityState::Unknown
        );
        assert_eq!(
            capability_state_from_ua_error(&err_with(None)),
            CapabilityState::Unknown
        );
        assert_eq!(
            capability_state_from_ua_error(&UaTransportError::timeout(
                UaOperation::Browse,
                "elapsed"
            )),
            CapabilityState::Unknown
        );
    }

    #[test]
    fn dv_string_rejects_bad_and_non_string() {
        // BAD 状态一票否决（即使值槽残留旧值）
        assert!(
            dv_string(&DataValue::new_now_status(
                "stale",
                StatusCode::BadNodeIdUnknown
            ))
            .is_none()
        );
        assert!(dv_string(&DataValue::new_now("  x  ")).as_deref() == Some("x"));
        assert!(dv_string(&DataValue::new_now("   ")).is_none());
        assert!(dv_string(&DataValue::new_now(42i32)).is_none());
    }
}
