//! SINUMERIK-shaped 确定性 fixture（PR11 Checkpoint D）。
//!
//! 定位：本 fixture 不假装"完整模拟真实 SINUMERIK"，它只冻结我们已经验证的
//! 设备语义假设，供 CI 确定性回归；真机（PR12）发现不一致时，按规则修 driver
//! 并把该事实补进本 fixture（真机事实 → driver → fixture regression → CI）。
//!
//! 冻结的假设（PR11）:
//! - 命名空间：`[标准, Siemens SINUMERIK]`（URI 形态，index 位置不假定）；
//! - BuildInfo：vendor `Siemens` / product `SINUMERIK 840D sl` / firmware `V5.24`；
//! - Objects 根可浏览（两页 + continuation 接力）；
//! - 若干可读标量（F64/String/I32，含 SourceTimestamp 语义由 Fake 赋予 `now`）。

use mesa_opcua_transport::{FakeLiveBatch, FakeOpcUaTransport, UaBrowsePage, UaNodeRef};

/// 标准 OPC UA 命名空间 URI（index 0，按规范恒定，fixture 显式声明）。
pub const STD_NS: &str = "http://opcfoundation.org/UA/";
/// Siemens SINUMERIK 命名空间 URI（PR11 假设形态，PR12 真机确认）。
pub const SIEMENS_NS: &str = "http://www.siemens.com/sinumerik";

pub const FIXTURE_VENDOR: &str = "Siemens";
pub const FIXTURE_MODEL: &str = "SINUMERIK 840D sl";
pub const FIXTURE_FIRMWARE: &str = "V5.24";

/// 标准 BuildInfo 节点（ns=0）：Manufacturer 2263 / Product 2261 / Firmware 2264。
pub const IDENT_MANUFACTURER: (u16, u32) = (0, 2263);
pub const IDENT_PRODUCT: (u16, u32) = (0, 2261);
pub const IDENT_FIRMWARE: (u16, u32) = (0, 2264);
/// Objects 根（ns=0;i=85）。
pub const OBJECTS_ROOT: (u16, u32) = (0, 85);

/// SINUMERIK 命名空间在 fixture 数组中的位置（仅 fixture 内使用；
/// driver 生产代码绝不假设 index，只认 URI）。
pub const SIEMENS_INDEX: u16 = 1;

/// fixture 命名空间数组。
pub fn namespace_array() -> Vec<String> {
    vec![STD_NS.to_string(), SIEMENS_NS.to_string()]
}

/// 确定性 SINUMERIK-shaped Fake transport：命名空间 + 身份 + 两页浏览 +
/// 三个标量读 + 一批订阅 live 事件。
pub fn sinumerik_shaped_fake() -> FakeOpcUaTransport {
    use mesa_opcua_transport::fake_browse_node;
    use opcua_types::DataValue;

    let token = vec![0xF1u8, 0xAA];
    FakeOpcUaTransport::new()
        .with_namespace_array(namespace_array())
        .with_read(
            &UaNodeRef::numeric(IDENT_MANUFACTURER.0, IDENT_MANUFACTURER.1),
            DataValue::new_now(FIXTURE_VENDOR),
        )
        .with_read(
            &UaNodeRef::numeric(IDENT_PRODUCT.0, IDENT_PRODUCT.1),
            DataValue::new_now(FIXTURE_MODEL),
        )
        .with_read(
            &UaNodeRef::numeric(IDENT_FIRMWARE.0, IDENT_FIRMWARE.1),
            DataValue::new_now(FIXTURE_FIRMWARE),
        )
        .with_browse_pages(
            &UaNodeRef::numeric(OBJECTS_ROOT.0, OBJECTS_ROOT.1),
            vec![
                UaBrowsePage {
                    nodes: vec![
                        fake_browse_node(UaNodeRef::string(SIEMENS_INDEX, "Channel"), "Channel"),
                        fake_browse_node(UaNodeRef::string(SIEMENS_INDEX, "Axis"), "Axis"),
                    ],
                    continuation_point: Some(token),
                },
                UaBrowsePage {
                    nodes: vec![fake_browse_node(
                        UaNodeRef::string(SIEMENS_INDEX, "Spindle"),
                        "Spindle",
                    )],
                    continuation_point: None,
                },
            ],
        )
        .with_read(
            &UaNodeRef::string(SIEMENS_INDEX, "Speed"),
            DataValue::new_now(1500.0f64),
        )
        .with_read(
            &UaNodeRef::string(SIEMENS_INDEX, "State"),
            DataValue::new_now("RUN"),
        )
        .with_read(
            &UaNodeRef::numeric(SIEMENS_INDEX, 7001),
            DataValue::new_now(42i32),
        )
        .with_live_batch(FakeLiveBatch {
            events: vec![(1, DataValue::new_now(43i32))],
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SinumerikConnConfig, SinumerikConnection, probe_with_transport};
    use mesa_core_types::{DataType, Quality, ValueOrigin};
    use mesa_driver_sdk::DriverConnection;
    use mesa_opcua_transport::OpcUaTransport;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// fixture 自检：probe 确认 SINUMERIK（high）+ browse 全 canonical +
    /// canonical→resolve→read→decode 一轮（冻结语义的回归锚点）。
    #[tokio::test]
    async fn fixture_freezes_sinumerik_read_only_semantics() {
        let fake = sinumerik_shaped_fake();
        // 1. Probe 确认
        let report = probe_with_transport(&fake).await.expect("probe Ok");
        assert!(report.reachable);
        assert_eq!(report.family.as_deref(), Some("SINUMERIK"));
        assert_eq!(report.model_confidence.as_deref(), Some("high"));
        assert_eq!(report.model.as_deref(), Some(FIXTURE_MODEL));

        // 2. Browse 全 canonical（含翻页聚合）
        let mut conn = SinumerikConnection::with_transport(
            SinumerikConnConfig::default(),
            Arc::new(sinumerik_shaped_fake()),
        );
        let (nodes, cursor) = conn.browse("", "", "", 50).await.expect("browse Ok");
        assert_eq!(nodes.len(), 3);
        assert!(cursor.is_none());
        for n in &nodes {
            assert!(
                n.id.starts_with(&format!("nsu={SIEMENS_NS};")),
                "非 canonical 身份: {}",
                n.id
            );
        }

        // 3. canonical → 当前 index → 读 → 解码（同一 fixture 内 index=1）
        let namespaces = fake.read_namespace_array().await.expect("ns Ok");
        let id = crate::parse_canonical(&format!("nsu={SIEMENS_NS};s=Speed")).expect("parse");
        let node = id.resolve(&namespaces).expect("resolve");
        assert_eq!(node, UaNodeRef::string(SIEMENS_INDEX, "Speed"));
        let vals = fake.read(std::slice::from_ref(&node)).await.expect("read");
        assert_eq!(vals.len(), 1);
        let spec = crate::PointSpec {
            key: "speed".into(),
            node: id,
            data_type: DataType::F64,
        };
        let mut cache = HashMap::new();
        let pv = crate::decode_data_value(&spec, 1, vals.into_iter().next().unwrap(), &mut cache);
        assert_eq!(pv.quality, Quality::Good);
        assert_eq!(pv.value_origin, ValueOrigin::Current);
        assert_eq!(pv.value, mesa_core_types::Value::F64(1500.0));
    }
}
