//! SINUMERIK NCK Driver — 原生 NCK 只读（ADR 0001，Commit B 空壳）。
//!
//! 架构位置：
//! ```text
//! sinumerik-nck（本 crate：NCK 设备语义，variable 资源）
//!       ↓ 不透明 var_spec（0x82/83/84，Commit C 起）
//! mesa-s7-transport（S7Comm 会话/ReadVar/分片）
//! ```
//!
//! - Core 只认识 Descriptor / ProbeReport / ResourceSelection / DataBatch，
//!   无任何 `driver_id == "sinumerik-nck"` 分支；
//! - V1 严格只读：`write`/`command` 沿用 SDK 默认 Unsupported；事件目录为空；
//! - 本空壳实现 descriptor + 连接配置校验；probe/configure/browse/run 全部
//!   `NOT_IMPLEMENTED`（TODO 注 Commit C/D/E，先让发现与契约就位）。

mod address;
mod catalog;
mod config;
mod fixture;

pub use address::{AddressError, NckArea, NckUnitMode, NckVariableRef};
pub use catalog::{CatalogError, NckCatalog, NckShape, NckVariableDefinition, NckWireDefinition};
pub use config::{NCK_DEFAULT_PORT, NckConnConfig};
pub use fixture::{NckFixtureState, spawn_nck_fixture};

use mesa_core_types::{AcquisitionTask, DriverMetadata, PointDescriptor, PointMap};
use mesa_driver_sdk::{Driver, DriverConnection, SdkDriverError};

/// 驱动 ID（Core 侧无分支；仅 Descriptor identity 与 driver.toml 声明）。
pub const DRIVER_ID: &str = "sinumerik-nck";
/// 通用轮询绑定（`mesa.resources.v1`，resource_id `variable`）。
pub const GENERIC_RESOURCE_ID: &str = "variable";

#[derive(Default)]
pub struct SinumerikNckDriver;

#[async_trait::async_trait]
impl Driver for SinumerikNckDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: DRIVER_ID.into(),
            name: "SINUMERIK NCK".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol_major: 1,
            protocol_minor: 0,
        }
    }

    fn descriptor(&self) -> mesa_core_types::DriverDescriptor {
        use mesa_core_types::{
            AccessMode, DataType, DiscoveryCapabilities, DriverCapabilities, DriverDescriptor,
            DriverIdentity, FieldDescriptor, FieldType, LocalizedText, OutputDescriptor,
            ResourceDescriptor, SchemaDescriptor,
        };
        let m = self.metadata();
        DriverDescriptor {
            contract_major: 1,
            contract_minor: 0,
            identity: DriverIdentity {
                driver_id: m.driver_id,
                name: m.name,
                version: m.version,
            },
            connection: SchemaDescriptor {
                fields: vec![
                    FieldDescriptor::new("host", "Host", FieldType::Host)
                        .required(true)
                        .default_value(serde_json::json!("192.168.0.1")),
                    FieldDescriptor::new("port", "Port", FieldType::Port)
                        .required(false)
                        .default_value(serde_json::json!(102)),
                    // TSAP 显式必填（NCK 无 rack/slot 推导；远端值由 profile + 真机 Gate 定）。
                    FieldDescriptor::new("local_tsap", "Local TSAP", FieldType::Integer)
                        .required(true),
                    FieldDescriptor::new("remote_tsap", "Remote TSAP", FieldType::Integer)
                        .required(true),
                    FieldDescriptor::new("timeout_ms", "Timeout ms", FieldType::Duration)
                        .required(false)
                        .default_value(serde_json::json!(5000)),
                    FieldDescriptor::new("pdu_length", "PDU length", FieldType::Integer)
                        .required(false)
                        .default_value(serde_json::json!(480)),
                ],
            },
            resources: vec![ResourceDescriptor {
                id: GENERIC_RESOURCE_ID.into(),
                label: LocalizedText::new("NC Variable"),
                parameters: SchemaDescriptor {
                    fields: vec![
                        {
                            let mut f = FieldDescriptor::new("area", "Area", FieldType::Enum)
                                .required(true)
                                .default_value(serde_json::json!("C"));
                            f.validation.enum_options = Some(vec![
                                "N".into(),
                                "B".into(),
                                "C".into(),
                                "A".into(),
                                "T".into(),
                                "V".into(),
                                "H".into(),
                            ]);
                            f
                        },
                        FieldDescriptor::new("area_no", "Area No.", FieldType::Integer)
                            .required(false),
                        FieldDescriptor::new("block", "Block", FieldType::String).required(true),
                        FieldDescriptor::new("variable", "Variable", FieldType::String)
                            .required(true),
                        FieldDescriptor::new("line", "Line", FieldType::Integer).required(false),
                        FieldDescriptor::new("column", "Column", FieldType::Integer)
                            .required(false),
                        FieldDescriptor::new("count", "Count", FieldType::Integer)
                            .required(false)
                            .default_value(serde_json::json!(1)),
                        {
                            let mut f =
                                FieldDescriptor::new("unit_mode", "Unit mode", FieldType::Enum)
                                    .required(false)
                                    .default_value(serde_json::json!("current"));
                            f.validation.enum_options =
                                Some(vec!["current".into(), "metric".into(), "inch".into()]);
                            f
                        },
                    ],
                },
                outputs: vec![OutputDescriptor {
                    id: "value".into(),
                    label: LocalizedText::new("Value"),
                    data_type: DataType::F64,
                    unit: None,
                    access: AccessMode::Read,
                }],
                modes: vec![mesa_core_types::TaskMode::Poll],
            }],
            controls: mesa_core_types::ControlCatalog::default(),
            discovery: DiscoveryCapabilities {
                manual: true,
                // Commit E 翻转为 true（Catalog + Topology 虚拟树就位后）。
                browse: false,
                import: false,
            },
            capabilities: DriverCapabilities {
                poll: true,
                // 不伪造 Subscribe（ReadVar 本质是请求/响应）；browse 能力随 Commit E。
                subscribe: false,
                browse: false,
                ..Default::default()
            },
            // Event Plane：NCK V1 无事件目录即 empty（Major 不升级）。
            events: Default::default(),
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        let v: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            SdkDriverError::configuration("BAD_CONFIG", format!("connection JSON 非法: {e}"))
        })?;
        let cfg = NckConnConfig::from_json(&v)?;
        Ok(Box::new(NckConnection { cfg }))
    }
}

#[derive(Debug)]
struct NckConnection {
    cfg: NckConnConfig,
}

fn not_implemented(what: &str) -> SdkDriverError {
    SdkDriverError::new(
        mesa_core_types::ErrorKind::Internal,
        "NOT_IMPLEMENTED",
        format!("sinumerik-nck {what} 尚未实现（experimental 空壳，见 GATE.md）"),
    )
}

#[async_trait::async_trait]
impl DriverConnection for NckConnection {
    // TODO(Commit C)：TCP → COTP → Setup → NCK 0x82 安全探测变量 → topology 摘要。
    // probe anchor 须从官方变量表选永久只读跨版本稳定变量，真机 Gate 冻结。
    async fn probe(&mut self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        let _ = &self.cfg;
        Err(not_implemented("probe"))
    }

    // TODO(Commit D)：ResourceSelection → NckVariableRef → Catalog → PointDescriptor。
    async fn configure(
        &mut self,
        _revision: u64,
        _tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        Err(not_implemented("configure"))
    }

    async fn apply_point_map(&mut self, _map: PointMap) -> Result<(), SdkDriverError> {
        Err(not_implemented("apply_point_map"))
    }

    // TODO(Commit D)：Poll + MultiRead + DataBatch + LastKnown + Stop。
    async fn run(
        &mut self,
        _sink: mesa_driver_sdk::DataSink,
        _shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<(), SdkDriverError> {
        Err(not_implemented("run"))
    }

    // TODO(Commit E)：Catalog + Topology 虚拟 Browse 树。
    async fn browse(
        &mut self,
        _parent: &str,
        _filter: &str,
        _cursor: &str,
        _limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), SdkDriverError> {
        Err(not_implemented("browse"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_valid_and_read_only() {
        let d = SinumerikNckDriver.descriptor();
        d.validate().expect("sinumerik-nck descriptor 必须合法");
        assert_eq!(d.identity.driver_id, "sinumerik-nck");
        assert!(d.capabilities.poll, "NCK V1 必须 poll");
        assert!(!d.capabilities.subscribe, "NCK 不伪造 subscribe");
        assert!(!d.capabilities.write, "NCK V1 只读");
        assert!(!d.capabilities.method, "NCK V1 无 command");
        assert!(!d.capabilities.events, "NCK V1 无事件");
        // variable 资源参数与 address.rs 口径一致。
        let res = d
            .resources
            .iter()
            .find(|r| r.id == "variable")
            .expect("variable 资源");
        let keys: Vec<_> = res
            .parameters
            .fields
            .iter()
            .map(|f| f.key.as_str())
            .collect();
        for k in [
            "area",
            "area_no",
            "block",
            "variable",
            "line",
            "column",
            "count",
            "unit_mode",
        ] {
            assert!(keys.contains(&k), "缺参数 {k}");
        }
    }

    #[tokio::test]
    async fn open_connection_validates_config() {
        async fn err_of(d: &SinumerikNckDriver, json: &str) -> SdkDriverError {
            match d.open_connection("t", json).await {
                Ok(_) => panic!("非法配置必须拒绝: {json}"),
                Err(e) => e,
            }
        }
        let d = SinumerikNckDriver;
        assert_eq!(err_of(&d, "not-json").await.code, "BAD_CONFIG");
        // 缺 TSAP 拒绝。
        assert_eq!(
            err_of(&d, r#"{"host":"10.0.0.5"}"#).await.code,
            "BAD_CONFIG"
        );
        let ok = d
            .open_connection(
                "t",
                r#"{"host":"10.0.0.5","local_tsap":256,"remote_tsap":258}"#,
            )
            .await;
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn runtime_methods_are_not_implemented_skeleton() {
        // 空壳门：运行期方法全部 NOT_IMPLEMENTED（Commit C/D/E 逐个点亮）。
        let d = SinumerikNckDriver;
        let mut conn = match d
            .open_connection(
                "t",
                r#"{"host":"10.0.0.5","local_tsap":256,"remote_tsap":258}"#,
            )
            .await
        {
            Ok(c) => c,
            Err(e) => panic!("open 必须 Ok，实际: {e}"),
        };
        assert_eq!(conn.probe().await.unwrap_err().code, "NOT_IMPLEMENTED");
        assert_eq!(
            conn.configure(1, vec![]).await.unwrap_err().code,
            "NOT_IMPLEMENTED"
        );
        assert_eq!(
            conn.browse("", "", "", 0).await.unwrap_err().code,
            "NOT_IMPLEMENTED"
        );
    }
}
