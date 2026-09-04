//! OPC UA 动态探测（V1.2.1 §8，feat/dynamic-probe 阶段 6）。
//!
//! 只允许经公共 [`mesa_opcua_transport::OpcUaTransport`] 调用，禁止直连
//! `async-opcua-client`（Stage 2 冻结边界；本文件无任何 async-opcua 导入）。
//! 探测内容：建连 → NamespaceArray → 标准 BuildInfo 身份 → Objects 浅浏览 → 断开。
//! 不建订阅（subscribe 本次不确认 → None），不写任何东西。

use mesa_core_types::{ProbeCapabilities, ProbeReport, ProbeWarning};
use mesa_driver_sdk::SdkDriverError;
use mesa_opcua_transport::{OpcUaTransport, UaBrowseRequest, UaNodeRef};

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
pub async fn probe_with_transport<T: OpcUaTransport>(
    transport: &T,
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

async fn probe_connected<T: OpcUaTransport>(transport: &T) -> ProbeReport {
    let mut warnings = Vec::new();

    // 1. NamespaceArray：读成功即证实 read 通路
    let read_ok = match transport.read_namespace_array().await {
        Ok(_) => true,
        Err(e) => {
            warnings.push(ProbeWarning {
                code: "NAMESPACE_READ_FAILED".into(),
                message: format!("NamespaceArray 读取失败: {e}"),
            });
            false
        }
    };

    // 2. 标准 BuildInfo 身份（逐点 BAD 容忍：单点缺失只影响对应字段）
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
            (get(0), get(1), get(2))
        }
        Err(e) => {
            warnings.push(ProbeWarning {
                code: "IDENTITY_UNAVAILABLE".into(),
                message: format!("BuildInfo 读取失败: {e}"),
            });
            (None, None, None)
        }
    };
    if vendor.is_none() && model.is_none() && firmware.is_none() {
        warnings.push(ProbeWarning {
            code: "MODEL_UNDETECTED".into(),
            message: "标准 BuildInfo 无可用身份信息".into(),
        });
    }

    // 3. Objects 浅浏览确认 browse 能力（单页即可，不翻页）
    let browse_ok = transport
        .browse(UaBrowseRequest {
            node: UaNodeRef::numeric(OBJECTS_ROOT.0, OBJECTS_ROOT.1),
            max_refs: 100,
        })
        .await
        .is_ok();
    if !browse_ok {
        warnings.push(ProbeWarning {
            code: "BROWSE_CHECK_FAILED".into(),
            message: "Objects 浅浏览失败".into(),
        });
    }

    ProbeReport {
        reachable: true,
        vendor,
        family: None,
        model,
        firmware,
        capabilities: ProbeCapabilities {
            read: Some(read_ok),
            // 本次未建订阅：按 Option<bool> 语义保持 None（未确认≠不支持）
            subscribe: None,
            browse: Some(browse_ok),
        },
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

    #[tokio::test]
    async fn full_identity_probed() {
        let t = ident_fake();
        let r = probe_with_transport(&t).await.expect("Ok");
        assert!(r.reachable);
        assert_eq!(r.vendor.as_deref(), Some("TestVendor"));
        assert_eq!(r.model.as_deref(), Some("TestModel"));
        assert_eq!(r.firmware.as_deref(), Some("9.9"));
        assert!(r.warnings.is_empty());
        assert_eq!(r.capabilities.read, Some(true));
        assert_eq!(r.capabilities.subscribe, None);
        assert_eq!(r.capabilities.browse, Some(true));
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
        assert_eq!(r.capabilities.read, Some(true));
    }

    #[tokio::test]
    async fn driver_probe_refused_port_is_unreachable() {
        // 127.0.0.1:9 预期关闭：建连失败 → Ok(unreachable)，不断开泄漏由内部保证
        let d = crate::OpcUaDriver;
        let r = mesa_driver_sdk::Driver::probe(
            &d,
            r#"{"endpoint_url":"opc.tcp://127.0.0.1:9","timeout_ms":1000}"#,
        )
        .await
        .expect("不可达是探测结果");
        assert!(!r.reachable);
        assert!(r.warnings.iter().any(|w| w.code == "CONNECTION_FAILED"));
    }

    #[tokio::test]
    async fn driver_probe_rejects_bad_config() {
        let d = crate::OpcUaDriver;
        let err = mesa_driver_sdk::Driver::probe(&d, "not-json")
            .await
            .unwrap_err();
        assert_eq!(err.code, "BAD_CONFIG");
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
