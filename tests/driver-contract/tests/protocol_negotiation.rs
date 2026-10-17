//! §21 Incompatible Version：协议 Major 不兼容时双方都必须明确拒绝（§14.3）。
//!
//! 使用裸帧客户端/服务端直接操纵 Hello/Welcome，验证协商规则本身。
//!
//! 本套件同时承载 I-2 阶段 1 的协议兼容断言（DC §8.3 的 I10/I27/I28/I34/I35
//! 与 §7.7 的 V1–V6）：additive 增补不改既有消息、Minor 门控先于发帧、
//! SDK 默认实现快速返回 Unsupported、未知消息不致命、旧消息集静默降级。

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mesa_core_types::ErrorKind;
use mesa_driver_manager::session::{Session, SessionError};
use mesa_driver_protocol::{
    DEFINITIONS_RPC_MIN_MINOR, PROTOCOL_MAJOR, PROTOCOL_MINOR, definitions_query_supported, pb,
    read_envelope, write_envelope,
};
use mesa_driver_sdk::{Driver, DriverConnection, SdkDriverError};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio_util::sync::CancellationToken;

use common::*;

/// 驱动声明更高 Major → Core 必须拒绝握手（Session 侧 negotiate 失败）。
#[tokio::test]
async fn core_rejects_incompatible_driver_major() {
    // 假驱动：§14.3 方向由 Driver 先发言 Hello，这里故意声明更高 Major。
    // Core 必须在协商期以 Handshake 错误拒绝，而不是干等超时。
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (_, mut wr) = sock.into_split();
        let env = pb::Envelope {
            msg_id: 1,
            body: Some(pb::envelope::Body::Hello(pb::Hello {
                driver_id: "bad-major".into(),
                driver_version: "0.0.0".into(),
                protocol_major: PROTOCOL_MAJOR + 1, // 制造 Major 不兼容
                protocol_minor: PROTOCOL_MINOR,
                sdk_version: "test".into(),
                platform: std::env::consts::OS.into(),
                instance_id: "bad-major-1".into(),
                session_token: TOKEN.into(),
            })),
        };
        let _ = write_envelope(&mut wr, &env).await;
        // 保持连接片刻，观察 Core 是否主动断开
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    let result = Session::connect(port, TOKEN).await;
    match result {
        Ok(_) => panic!("incompatible major must be rejected"),
        Err(e) => assert_handshake_error(e),
    }
}

/// 对端 accept 后永远沉默 → Hello 等待超时是 Timeout（语义真相），
/// 不是 Handshake 文本错误；调用方（含 probe startup 分类）据此精确路由。
#[tokio::test]
async fn silent_driver_hello_wait_is_timeout() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _sock = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let result = Session::connect(port, TOKEN).await;
    match result {
        Ok(_) => panic!("silent driver must time out"),
        Err(e) => assert!(
            matches!(e, mesa_driver_manager::session::SessionError::Timeout),
            "hello 等待超时必须是 Timeout 变体，实际: {e}"
        ),
    }
}

/// Core 回 Welcome 声明更低 accepted_major → 驱动侧 serve 必须报错退出，
/// 而不是带着不兼容协议继续运行。
#[tokio::test]
async fn driver_rejects_core_major_downgrade() {
    // 进程内启动真实 SDK server
    let sim_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = sim_listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let server = tokio::spawn(async move {
        mesa_driver_sdk::serve(mesa_test_driver::TestDriver, sim_listener, TOKEN.into(), c).await
    });

    // 裸 Core 客户端：读 Hello 后回一个 Major 不一致的 Welcome
    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut rd, mut wr) = sock.into_split();
    let hello = read_envelope(&mut rd).await.expect("sim sends hello first");
    let welcome = pb::Envelope {
        msg_id: hello.msg_id,
        body: Some(pb::envelope::Body::Welcome(pb::Welcome {
            core_version: "test-core".into(),
            accepted_protocol_major: PROTOCOL_MAJOR - 1, // 强制不兼容
            accepted_protocol_minor: PROTOCOL_MINOR,
        })),
    };
    write_envelope(&mut wr, &welcome).await.unwrap();

    let joined = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("serve must finish");
    let result = joined.expect("join serve task");
    let err = result.expect_err("serve must return Err on incompatible major");
    let msg = err.to_string();
    assert!(
        msg.contains("major") || msg.contains("rejected"),
        "error must mention version rejection: {msg}"
    );
}

/// 真 1.2 驱动（裸帧 Hello{1,2}，不经当前 SDK）：
/// Welcome 必须如实回 accepted_minor=2（协商值，不是 Core 自身 3）；
/// 该会话的 Event Gate 必须以协商值 2 执行（非空任务秒级精确拒绝）。
#[tokio::test]
async fn minor_negotiation_with_real_1_2_driver_is_honest() {
    use mesa_core_types::{DriverBinding, EventTask, TaskMode};

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // 假 1.2 驱动：发 Hello{1,2} → 校验 Welcome → 保持连接供后续 RPC 门控验证
    let stub = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = sock.into_split();
        let hello = pb::Envelope {
            msg_id: 1,
            body: Some(pb::envelope::Body::Hello(pb::Hello {
                driver_id: "legacy-1.2".into(),
                driver_version: "1.2.0".into(),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: 2,
                sdk_version: "test".into(),
                platform: "test".into(),
                instance_id: "test-1".into(),
                session_token: TOKEN.into(),
            })),
        };
        write_envelope(&mut wr, &hello).await.unwrap();
        let welcome = read_envelope(&mut rd)
            .await
            .expect("core must answer Welcome");
        let w = match welcome.body {
            Some(pb::envelope::Body::Welcome(w)) => w,
            other => panic!("expected Welcome, got {other:?}"),
        };
        assert_eq!(w.accepted_protocol_major, PROTOCOL_MAJOR);
        assert_eq!(
            w.accepted_protocol_minor, 2,
            "Welcome 必须回协商 Minor=2，不能回 Core 自身 Minor"
        );
        // 保持连接：心跳 Ping 到达即回 Pong（让会话在断言期间存活）
        loop {
            let env = match read_envelope(&mut rd).await {
                Ok(e) => e,
                Err(_) => break,
            };
            if matches!(env.body, Some(pb::envelope::Body::Ping(_))) {
                let pong = pb::Envelope {
                    msg_id: env.msg_id,
                    body: Some(pb::envelope::Body::Pong(pb::Pong {})),
                };
                if write_envelope(&mut wr, &pong).await.is_err() {
                    break;
                }
            }
        }
    });

    let (mut session, _events, _) = Session::connect(port, TOKEN)
        .await
        .expect("1.2 handshake ok");
    assert_eq!(session.negotiated_minor(), 2, "协商值必须为双方较小值 2");
    // wire 级门控：该会话发非空事件任务必须秒级精确拒绝（不发未知 RPC 干等）
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        session.configure_events(
            7,
            1,
            &[EventTask {
                id: "e1".into(),
                mode: TaskMode::Subscribe,
                interval_ms: None,
                binding: DriverBinding {
                    kind: "k".into(),
                    config: serde_json::Value::Null,
                },
            }],
        ),
    )
    .await
    .expect("1.2 gate must answer fast")
    .expect_err("1.2 driver must reject event tasks");
    assert!(
        matches!(
            err,
            SessionError::EventPlaneUnsupported {
                negotiated: 2,
                required: 3
            }
        ),
        "必须精确报 EventPlaneUnsupported，got {err}"
    );

    teardown(&mut session, None);
    stub.abort();
}

// ===========================================================================
// I-2 阶段 1：协议 additive 增补 / Minor 门控 / SDK 默认实现 / 非 NCK 兼容
//
// 本节承载 DC §8.3 的 I10、I27、I28、I34、I35 与 §7.7 版本兼容矩阵的 V1–V6。
// 全部使用进程内 SDK 服务 + 裸帧注入，**不连接任何真实设备、不启动 mesad**。
//
// 分工说明（与阶段 2 的边界）：
// - 本节证明的是**协议与 SDK 层**的契约：wire 形状、门控语义、默认实现的快速
//   Unsupported、未知消息不致命、旧消息集降级；
// - 「Manager 侧调用门控并映射成 HTTP 501」属阶段 2（M4/M5/N3），本节只断言
//   门控函数本身与「不发帧」这一可观测事实。
// ===========================================================================

// ---------------------------------------------------------------------------
// I27：proto diff 只体现新增（断言对象是 §7.2.1 的逐字段表，不是数字）
// ---------------------------------------------------------------------------

/// §7.2.1 的 35 行逐字段核对表（tag → 成员类型 → 成员名）＝**增补前的真值**。
///
/// 取值方式：对增补前的 `proto/driver.proto`（基线 `e250600`）逐行解析
/// `oneof body` 得到，**不是**从改动后的文件反推 —— 否则这条断言会变成
/// 自证。DC §7.2.1 的口径：「数字（35/44）是派生物，逐字段表才是真值」。
const FROZEN_ONEOF_BODY: [(u32, &str, &str); 35] = [
    (10, "Hello", "hello"),
    (11, "Welcome", "welcome"),
    (12, "GetMetadata", "get_metadata"),
    (13, "MetadataReport", "metadata_report"),
    (14, "OpenConnection", "open_connection"),
    (15, "OpenConnectionAck", "open_connection_ack"),
    (16, "CloseConnection", "close_connection"),
    (17, "CloseConnectionAck", "close_connection_ack"),
    (18, "ConfigureTasks", "configure_tasks"),
    (19, "PointDescriptorsReport", "point_descriptors"),
    (20, "ApplyPointMap", "apply_point_map"),
    (21, "ConfigApplied", "config_applied"),
    (22, "StartConnection", "start_connection"),
    (23, "StartConnectionAck", "start_connection_ack"),
    (24, "StopConnection", "stop_connection"),
    (25, "StopConnectionAck", "stop_connection_ack"),
    (26, "ConnectionStateChanged", "connection_state_changed"),
    (27, "DataBatchMsg", "data_batch"),
    (28, "DriverErrorReport", "driver_error"),
    (29, "Ping", "ping"),
    (30, "Pong", "pong"),
    (31, "Shutdown", "shutdown"),
    (32, "GetDescriptor", "get_descriptor"),
    (33, "DescriptorReport", "descriptor_report"),
    (34, "BrowseRequest", "browse_request"),
    (35, "BrowseResponse", "browse_response"),
    (36, "WriteRequest", "write_request"),
    (37, "WriteResponse", "write_response"),
    (38, "CommandRequest", "command_request"),
    (39, "CommandResponse", "command_response"),
    (40, "ProbeRequest", "probe_request"),
    (41, "ProbeResponse", "probe_response"),
    (42, "ConfigureEventTasks", "configure_event_tasks"),
    (43, "EventConfigApplied", "event_config_applied"),
    (44, "EventBatchMsg", "event_batch"),
];

/// 全部既有 message 的**直接字段**逐字快照（同样取自增补前的文件）。
///
/// 口径（DC §8.3-I27-④）：既有 message 的字段**编号与类型**逐字不变。
/// 这里保存的是逐字文本（含 `optional`/`repeated`/`map<...>`/`[deprecated]`
/// 选项），因此任何改名、改号、改类型、增删字段都会被这条断言拦住。
/// `Envelope` 只含 `msg_id`（其 `oneof body` 成员由上面的表单独断言）。
const FROZEN_MESSAGE_FIELDS: &[&str] = &[
    "Envelope: uint64 msg_id = 1",
    "Hello: string driver_id = 1 | string driver_version = 2 | uint32 protocol_major = 3 | uint32 protocol_minor = 4 | string sdk_version = 5 | string platform = 6 | string instance_id = 7 | string session_token = 8",
    "Welcome: string core_version = 1 | uint32 accepted_protocol_major = 2 | uint32 accepted_protocol_minor = 3",
    "GetMetadata:",
    "MetadataReport: string driver_id = 1 | string name = 2 | string version = 3",
    "GenericResult: bool ok = 1 | ErrorDetail error = 2",
    "ErrorDetail: string kind = 1 | string code = 2 | string message = 3",
    "OpenConnection: uint32 connection_handle = 1 | string endpoint_id = 2 | string config_json = 3",
    "OpenConnectionAck: uint32 connection_handle = 1 | GenericResult result = 2",
    "CloseConnection: uint32 connection_handle = 1",
    "CloseConnectionAck: uint32 connection_handle = 1 | GenericResult result = 2",
    "ConfigureTasks: uint32 connection_handle = 1 | uint64 revision = 2 | repeated AcquisitionTaskProto tasks = 3",
    "AcquisitionTaskProto: string id = 1 | string mode = 2 | optional uint64 interval_ms = 3 | string binding_kind = 4 | string binding_config_json = 5 | optional uint64 publishing_interval_ms = 6 | optional uint64 sampling_interval_ms = 7 | optional uint32 queue_size = 8 | optional bool discard_oldest = 9",
    "PointDescriptorsReport: uint32 connection_handle = 1 | uint64 revision = 2 | repeated PointDescriptorProto descriptors = 3",
    "PointDescriptorProto: string point_key = 1 | string data_type = 2 | optional string unit = 3 | optional string source_label = 4",
    "ApplyPointMap: uint32 connection_handle = 1 | uint64 revision = 2 | map<string, uint32> key_to_point_id = 3",
    "ConfigApplied: uint32 connection_handle = 1 | uint64 revision = 2 | GenericResult result = 3",
    "StartConnection: uint32 connection_handle = 1 | uint64 stream_epoch = 2",
    "StartConnectionAck: uint32 connection_handle = 1 | GenericResult result = 2",
    "StopConnection: uint32 connection_handle = 1",
    "StopConnectionAck: uint32 connection_handle = 1 | GenericResult result = 2",
    "ConnectionStateChanged: uint32 connection_handle = 1 | string state = 2 | string detail = 3",
    "DataBatchMsg: uint32 connection_handle = 1 | uint64 stream_epoch = 2 | uint64 sequence = 3 | int64 timestamp_ns = 4 | repeated PointValueMsg values = 5 | optional uint64 mono_ns = 6",
    "PointValueMsg: uint32 point_id = 1 | ValueMsg value = 2 | string quality = 3 | optional sint32 quality_code = 4 | optional int64 source_timestamp_ns = 5 | ValueOrigin value_origin = 6",
    "ValueMsg:",
    "BoolArrayV: repeated bool v = 1",
    "I32ArrayV: repeated sint32 v = 1",
    "U32ArrayV: repeated uint32 v = 1",
    "I64ArrayV: repeated sint64 v = 1",
    "U64ArrayV: repeated uint64 v = 1",
    "F32ArrayV: repeated float v = 1",
    "F64ArrayV: repeated double v = 1",
    "StringArrayV: repeated string v = 1",
    "DateTimeArrayV: repeated int64 v = 1",
    "DriverErrorReport: optional uint32 connection_handle = 1 | ErrorDetail detail = 2",
    "Ping:",
    "Pong:",
    "Shutdown:",
    "GetDescriptor:",
    "DescriptorReport: uint32 contract_major = 1 | uint32 contract_minor = 2 | string descriptor_json = 3",
    "BrowseRequest: uint32 connection_handle = 1 | string parent = 2 | string filter = 3 | string cursor = 4 | uint32 limit = 5",
    "BrowseNode: string id = 1 | string label = 2 | string kind = 3 | string data_type = 4 | string access = 5 | bool has_children = 6 | string binding_json = 7",
    "BrowseResponse: repeated BrowseNode nodes = 1 | string next_cursor = 2 | GenericResult result = 3",
    "WriteRequest: uint32 connection_handle = 1 | string request_id = 2 | string target = 3 [deprecated = true] | ValueMsg value = 4 | optional ValueMsg expected_value = 5 | string resource_id = 6 | string parameters_json = 7 | string output = 8",
    "WriteResponse: string request_id = 1 | GenericResult result = 2 | optional ValueMsg readback = 3",
    "CommandRequest: uint32 connection_handle = 1 | string request_id = 2 | string command_id = 3 | string input_json = 4",
    "CommandResponse: string request_id = 1 | string status = 2 | string result_json = 3 | string error = 4",
    "ProbeRequest: uint32 connection_handle = 1",
    "ProbeResponse: string report_json = 1",
    "EventTaskProto: string id = 1 | string mode = 2 | optional uint64 interval_ms = 3 | string binding_kind = 4 | string binding_config_json = 5",
    "ConfigureEventTasks: uint32 connection_handle = 1 | uint64 revision = 2 | repeated EventTaskProto tasks = 3",
    "EventConfigApplied: uint32 connection_handle = 1 | uint64 revision = 2 | GenericResult result = 3",
    "EventAttributeMsg: string key = 1 | ValueMsg value = 2",
    "EventConditionMsg: string condition_id = 1 | string transition = 2 | optional bool active = 3 | optional bool acknowledged = 4 | optional bool confirmed = 5 | optional bool retain = 6",
    "EventRecordMsg: string event_id = 1 | string category = 2 | string kind = 3 | string source = 4 | uint32 severity = 5 | optional string code = 6 | optional string message = 7 | optional string message_locale = 8 | optional int64 occurred_at_ns = 9 | optional EventConditionMsg condition = 10 | optional string correlation_id = 11 | repeated EventAttributeMsg attributes = 12",
    "EventBatchMsg: uint32 connection_handle = 1 | uint64 stream_epoch = 2 | uint64 sequence = 3 | int64 timestamp_ns = 4 | optional uint64 mono_ns = 5 | repeated EventRecordMsg events = 6",
];

/// I-2 新增的 4 个 `oneof body` 成员（tag 45–48，DC §8.2）。
const ADDITIVE_ONEOF_BODY: [(u32, &str, &str); 4] = [
    (45, "GetDefinitions", "get_definitions"),
    (46, "DefinitionsReport", "definitions_report"),
    (47, "PreviewPointKey", "preview_point_key"),
    (48, "PreviewIdentityReport", "preview_identity_report"),
];

/// I-2 新增的 4 个 message 的字段形状（DC §8.2 逐字）。
const ADDITIVE_MESSAGES: [(&str, &[&str]); 4] = [
    // 目录查询**必须**带请求参数：Manager 的签名是 definitions(driver_id, query_json)，
    // 且 UNSUPPORTED_FAMILY / UNKNOWN_VARIABLE / UNSUPPORTED_DEFINITION_REVISION /
    // INVALID_QUERY 的归属方是 Driver 层 ⇒ 参数在 wire 上必须有承载位置。
    ("GetDefinitions", &["string request_json = 1"]),
    (
        "DefinitionsReport",
        &["GenericResult result = 1", "string definitions_json = 2"],
    ),
    ("PreviewPointKey", &["string request_json = 1"]),
    (
        "PreviewIdentityReport",
        &["GenericResult result = 1", "string result_json = 2"],
    ),
];

/// proto 块栈：只区分消息 / 枚举 / oneof 三类（本文件没有嵌套消息）。
enum ProtoBlock {
    Message(String),
    Enum,
    Oneof(String),
}

/// proto 解析结果：message 声明顺序 + 字段逐字行、`Envelope.body` 成员表、oneof 总数。
struct ProtoModel {
    messages: Vec<(String, Vec<String>)>,
    oneof_body: Vec<(u32, String, String)>,
    oneof_count: usize,
}

/// 拆解一行字段声明 → (label, 类型, 名称, tag, 选项)。
///
/// 为什么不用正则：测试 crate 未依赖 regex，且这里只需要「最后一个空白分隔
/// token 是字段名」这一条规则 —— 它恰好也能正确处理含空格的
/// `map<string, uint32> key_to_point_id`。
fn split_field(line: &str) -> (String, String, String, u32, String) {
    let line = line.trim().trim_end_matches(';').trim();
    let (left, right) = line
        .split_once('=')
        .unwrap_or_else(|| panic!("字段行缺 '=': {line}"));
    let right = right.trim();
    let digits: String = right.chars().take_while(|c| c.is_ascii_digit()).collect();
    let tag: u32 = digits
        .parse()
        .unwrap_or_else(|_| panic!("字段 tag 不是数字: {line}"));
    let opts = right[digits.len()..].trim().to_string();
    let left = left.trim();
    let (label, rest) = if let Some(r) = left.strip_prefix("optional ") {
        ("optional".to_string(), r.trim())
    } else if let Some(r) = left.strip_prefix("repeated ") {
        ("repeated".to_string(), r.trim())
    } else {
        (String::new(), left)
    };
    let idx = rest
        .rfind(char::is_whitespace)
        .unwrap_or_else(|| panic!("字段行缺类型或名称: {line}"));
    let ty = rest[..idx].trim().to_string();
    let name = rest[idx..].trim().to_string();
    (label, ty, name, tag, opts)
}

/// 字段行的规范形态：`[optional|repeated ]<类型> <名称> = <tag>[ 选项]`。
fn canonical_field(line: &str) -> String {
    let (label, ty, name, tag, opts) = split_field(line);
    let mut s = String::new();
    if !label.is_empty() {
        s.push_str(&label);
        s.push(' ');
    }
    s.push_str(&format!("{ty} {name} = {tag}"));
    if !opts.is_empty() {
        s.push(' ');
        s.push_str(&opts);
    }
    s
}

/// 解析 `proto/driver.proto`（只处理本文件用到的语法子集：message / enum /
/// oneof / 单行空消息 / 单行包装消息 / 注释）。
fn parse_driver_proto(src: &str) -> ProtoModel {
    let mut stack: Vec<ProtoBlock> = Vec::new();
    let mut messages: Vec<(String, Vec<String>)> = Vec::new();
    let mut oneof_body: Vec<(u32, String, String)> = Vec::new();
    let mut oneof_count = 0usize;

    for raw in src.lines() {
        // 注释剥离（本文件不含带 `//` 的字符串字面量）
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(head) = line.strip_suffix('{') {
            let head = head.trim();
            let mut it = head.split_whitespace();
            let kind = it.next().unwrap_or("");
            let name = it.next().unwrap_or("");
            match kind {
                "message" => {
                    // 只含 oneof 的 message（Envelope / ValueMsg）也要登记，
                    // 否则「字段数为 0」与「消息不存在」无法区分
                    messages.push((name.to_string(), Vec::new()));
                    stack.push(ProtoBlock::Message(name.to_string()));
                }
                "enum" => stack.push(ProtoBlock::Enum),
                "oneof" => {
                    oneof_count += 1;
                    stack.push(ProtoBlock::Oneof(name.to_string()));
                }
                other => panic!("无法识别的块起始: {other} ({line})"),
            }
            continue;
        }
        // 单行形态：`message Ping {}` / `message BoolArrayV { repeated bool v = 1; }`
        if line.starts_with("message ") && line.ends_with('}') {
            let open = line.find('{').expect("单行 message 必须有 '{'");
            let name = line[..open].trim().trim_start_matches("message").trim();
            let inner = line[open + 1..line.len() - 1].trim();
            let fields = if inner.is_empty() {
                Vec::new()
            } else {
                vec![canonical_field(inner)]
            };
            messages.push((name.to_string(), fields));
            continue;
        }
        if line == "}" {
            stack.pop().expect("多余的 '}'");
            continue;
        }
        if stack.is_empty() && (line.starts_with("syntax") || line.starts_with("package")) {
            continue;
        }
        // enum 体（ValueOrigin）不是消息字段
        if stack.iter().any(|b| matches!(b, ProtoBlock::Enum)) {
            continue;
        }
        if let Some(ProtoBlock::Oneof(oname)) = stack.last() {
            let owner = stack.iter().rev().find_map(|b| match b {
                ProtoBlock::Message(n) => Some(n.as_str()),
                _ => None,
            });
            if oname == "body" && owner == Some("Envelope") {
                let (label, ty, name, tag, _) = split_field(line);
                assert!(label.is_empty(), "oneof 成员不应带 label: {line}");
                oneof_body.push((tag, ty, name));
            }
            continue;
        }
        let canonical = canonical_field(line);
        let owner = stack
            .iter()
            .rev()
            .find_map(|b| match b {
                ProtoBlock::Message(n) => Some(n.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("字段不在 message 内: {line}"));
        messages
            .iter_mut()
            .find(|(n, _)| *n == owner)
            .expect("owner 必须已登记")
            .1
            .push(canonical);
    }
    ProtoModel {
        messages,
        oneof_body,
        oneof_count,
    }
}

/// I27（DC §8.3，**修正口径**）：proto 增补只体现新增。
///
/// ① §7.2.1 的 35 行逐字段核对表逐字不变；② 既有 tag 集合恰为 `{10..44}`
/// （连续、无空洞、无重复）；③ 新增 tag 集合恰为 `{45,46,47,48}` 且新增 4 个
/// message；④ 全部既有 message 的字段编号/类型逐字不变；⑤ `GetDefinitions`
/// 恰 1 个字段 `request_json`（string = 1）——请求参数必须在 wire 上有承载位置。
///
/// **不得**把断言写成「44 个」（第 1 轮的计数错误），也不断言「39 个」这类
/// 派生数字以外的口径 —— 真值是逐字段表。
#[test]
fn protocol_additive_fields_do_not_alter_existing_messages() {
    let path = repo_root().join("proto").join("driver.proto");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()));
    let model = parse_driver_proto(&src);

    // ① 既有 35 行逐字段核对表逐字不变
    let frozen: Vec<(u32, String, String)> = FROZEN_ONEOF_BODY
        .iter()
        .map(|(t, ty, n)| (*t, (*ty).to_string(), (*n).to_string()))
        .collect();
    assert_eq!(
        model.oneof_body[..frozen.len()].to_vec(),
        frozen,
        "§7.2.1 的 35 行逐字段核对表（tag → 类型 → 名称）必须逐字不变"
    );

    // ② 既有 tag 编号集合恰为 {10..44}：连续、无空洞、无重复
    let tags: Vec<u32> = model.oneof_body.iter().map(|(t, _, _)| *t).collect();
    assert_eq!(
        tags[..35].to_vec(),
        (10u32..=44).collect::<Vec<_>>(),
        "既有 35 个 tag 必须仍是 10..44 连续无空洞"
    );
    let mut sorted = tags.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), tags.len(), "oneof body 不得出现重复 tag");

    // ③ 新增 tag 集合恰为 {45,46,47,48}
    assert_eq!(
        model.oneof_body.len(),
        39,
        "增补后 oneof body 成员数 = 35 + 4（数字是派生物，逐字段表才是真值）"
    );
    let added: Vec<(u32, String, String)> = model.oneof_body[35..].to_vec();
    let expected_added: Vec<(u32, String, String)> = ADDITIVE_ONEOF_BODY
        .iter()
        .map(|(t, ty, n)| (*t, (*ty).to_string(), (*n).to_string()))
        .collect();
    assert_eq!(
        added, expected_added,
        "新增 tag 集合必须恰为 {{45,46,47,48}}"
    );

    // ④ 全部既有 message 的字段逐字不变 + 新增 4 个 message 形状正确
    let mut existing: Vec<String> = Vec::new();
    let mut additive: Vec<(String, Vec<String>)> = Vec::new();
    for (name, fields) in &model.messages {
        if ADDITIVE_MESSAGES.iter().any(|(n, _)| n == name) {
            additive.push((name.clone(), fields.clone()));
        } else if fields.is_empty() {
            existing.push(format!("{name}:"));
        } else {
            existing.push(format!("{name}: {}", fields.join(" | ")));
        }
    }
    assert_eq!(
        model.messages.len(),
        60,
        "顶层 message 数 = 56 + 4（含 Envelope）"
    );

    // ⑤ `GetDefinitions` 的请求参数必须**在 wire 上有承载位置**：恰 1 个字段，
    //    名为 `request_json`、类型 `string`、编号 `1`。
    //
    // 为什么单独断言、而不是只靠下面的 `ADDITIVE_MESSAGES` 字符串表：目录查询参数
    //（family / variable_id / revision / limit / cursor）若在 wire 上无位置，驱动层
    // 的 `UNSUPPORTED_FAMILY` / `UNKNOWN_VARIABLE` / `UNSUPPORTED_DEFINITION_REVISION`
    // / `INVALID_QUERY` 就**永远无法触发**，Core 只能被迫自己解释 NCK 语义
    //（违反「Core 不懂协议」）。这条断言把「请求参数必须过线」钉成契约。
    let get_definitions = additive
        .iter()
        .find(|(n, _)| n == "GetDefinitions")
        .expect("必须存在 GetDefinitions 消息");
    assert_eq!(
        get_definitions.1.len(),
        1,
        "GetDefinitions 必须恰有 1 个字段（request_json），实测 {:?}",
        get_definitions.1
    );
    let (label, ty, name, tag, opts) = split_field(&get_definitions.1[0]);
    assert!(
        label.is_empty(),
        "request_json 不得带 label，实测 {label:?}"
    );
    assert!(
        opts.is_empty(),
        "request_json 不得带额外选项，实测 {opts:?}"
    );
    assert_eq!(
        name, "request_json",
        "GetDefinitions 的唯一字段名必须是 request_json"
    );
    assert_eq!(
        ty, "string",
        "request_json 必须是 string（不透明 JSON 文本，Core 不解释语义）"
    );
    assert_eq!(tag, 1, "request_json 的字段编号必须是 1");

    assert_eq!(
        existing,
        FROZEN_MESSAGE_FIELDS
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>(),
        "既有 message 的字段编号/类型必须逐字不变"
    );
    for (name, expected) in ADDITIVE_MESSAGES {
        let found = additive
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("缺少新增 message {name}"));
        assert_eq!(
            found.1, expected,
            "新增 message {name} 的字段形状必须与设计逐字一致"
        );
    }

    // 旁证：全文件仍只有 2 个 oneof（Envelope.body + ValueMsg.kind）
    assert_eq!(model.oneof_count, 2, "oneof 数不得因增补而变化");
}

// ---------------------------------------------------------------------------
// 夹具：旧驱动替身（Minor 6）、裸帧 Core 替身、自定义驱动服务
// ---------------------------------------------------------------------------

/// 帧的「名字」：用于记录旧驱动替身收到了什么。
/// 未知 tag（信封层无法识别）会落到 `None`，与旧 SDK 的兜底分支同语义。
fn frame_name(body: &Option<pb::envelope::Body>) -> String {
    match body {
        None => "Unknown".into(),
        Some(pb::envelope::Body::Ping(_)) => "Ping".into(),
        Some(pb::envelope::Body::Pong(_)) => "Pong".into(),
        Some(pb::envelope::Body::GetDescriptor(_)) => "GetDescriptor".into(),
        Some(pb::envelope::Body::DescriptorReport(_)) => "DescriptorReport".into(),
        Some(pb::envelope::Body::GetDefinitions(_)) => "GetDefinitions".into(),
        Some(pb::envelope::Body::PreviewPointKey(_)) => "PreviewPointKey".into(),
        Some(pb::envelope::Body::Welcome(w)) => {
            format!("Welcome(minor={})", w.accepted_protocol_minor)
        }
        Some(other) => format!("{other:?}"),
    }
}

/// 起一个「旧驱动二进制」替身：Hello 声明 `protocol_minor = 6`，
/// 握手后**只**回 Pong，其余帧一律记录并**静默忽略**。
///
/// 为什么要替身而不是拿旧二进制：本仓库只有当前 SDK，历史二进制不可得；
/// 旧 SDK 对未知消息的行为就是兜底分支「记 debug 日志并忽略」
///（`driver-sdk` 的 `unexpected envelope ignored`），替身逐字复刻这一语义，
/// 因此可以用来验证「发帧 = 白等」。
///
/// 返回 (端口, 收到的帧名序列, 停机句柄)。
async fn start_legacy_minor6_driver() -> (u16, Arc<Mutex<Vec<String>>>, CancellationToken) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log_in_task = log.clone();
    // 注意：任务不监听 cancel —— 它随连接关闭（读帧出错）自然退出；
    // 测试侧 `teardown` 的 invalidate 会关闭 socket，从而结束该任务。
    let cancel = CancellationToken::new();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = sock.into_split();
        let hello = pb::Envelope {
            msg_id: 1,
            body: Some(pb::envelope::Body::Hello(pb::Hello {
                driver_id: "legacy-minor-6".into(),
                driver_version: "1.6.0".into(),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: 6,
                sdk_version: "legacy-test".into(),
                platform: "test".into(),
                instance_id: "legacy-minor-6".into(),
                session_token: TOKEN.into(),
            })),
        };
        if write_envelope(&mut wr, &hello).await.is_err() {
            return;
        }
        let welcome = match read_envelope(&mut rd).await {
            Ok(e) => e,
            Err(_) => return,
        };
        log_in_task.lock().unwrap().push(frame_name(&welcome.body));
        loop {
            let env = match read_envelope(&mut rd).await {
                Ok(e) => e,
                Err(_) => return,
            };
            log_in_task.lock().unwrap().push(frame_name(&env.body));
            if matches!(env.body, Some(pb::envelope::Body::Ping(_))) {
                let pong = pb::Envelope {
                    msg_id: env.msg_id,
                    body: Some(pb::envelope::Body::Pong(pb::Pong {})),
                };
                if write_envelope(&mut wr, &pong).await.is_err() {
                    return;
                }
            }
            // 其余消息：旧 SDK 的兜底分支只记 debug 日志并忽略（不 panic、不断连）
        }
    });
    (port, log, cancel)
}

/// 裸帧 Core 替身：连上驱动、读 Hello、回 `accepted_minor` 的 Welcome。
///
/// 用裸帧而不是 `Session`：只有裸帧才能注入**未知 tag**（`Session` 只能发
/// 已生成的 oneof 变体）。
async fn raw_core_handshake(port: u16, accepted_minor: u32) -> (OwnedReadHalf, OwnedWriteHalf) {
    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut rd, mut wr) = sock.into_split();
    let hello = read_envelope(&mut rd).await.expect("driver 必须先发 Hello");
    match &hello.body {
        Some(pb::envelope::Body::Hello(h)) => {
            assert_eq!(h.protocol_major, PROTOCOL_MAJOR, "新驱动必须声明 Major 1");
            assert_eq!(
                h.protocol_minor, PROTOCOL_MINOR,
                "新驱动必须声明当前 Minor（否则协商值会莫名变小）"
            );
        }
        other => panic!("期望 Hello，实际 {other:?}"),
    }
    let welcome = pb::Envelope {
        msg_id: hello.msg_id,
        body: Some(pb::envelope::Body::Welcome(pb::Welcome {
            core_version: "test-core".into(),
            accepted_protocol_major: PROTOCOL_MAJOR,
            accepted_protocol_minor: accepted_minor,
        })),
    };
    write_envelope(&mut wr, &welcome).await.unwrap();
    (rd, wr)
}

/// 起一个进程内 SDK 服务（任意驱动实现），返回 (端口, 停机句柄)。
async fn start_driver_server<D: Driver>(driver: D) -> (u16, CancellationToken) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        if let Err(e) = mesa_driver_sdk::serve(driver, listener, TOKEN.into(), c).await {
            eprintln!("test driver server ended: {e}");
        }
    });
    (port, cancel)
}

/// 测试夹具：**实现了** I-2 两个 RPC 的驱动（"实现方"路径）。
///
/// 为什么放在测试文件里而不是 `tests/support/test-driver`：后者的源码在 I-2 中
/// 冻结（只允许同步 `driver.toml` 的 minor），所以「实现方」夹具必须自带。
struct DefinitionsDriver {
    catalog: &'static str,
}

#[async_trait::async_trait]
impl Driver for DefinitionsDriver {
    fn metadata(&self) -> mesa_core_types::DriverMetadata {
        mesa_core_types::DriverMetadata {
            driver_id: "definitions-fixture".into(),
            name: "Definitions Fixture".into(),
            version: "0.0.0".into(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        _config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        // I-2 的两条 RPC 不需要 Connection：本夹具刻意不实现连接，
        // 以此证明「零 Connection 条目也能应答」
        Err(SdkDriverError::new(
            ErrorKind::Unsupported,
            "UNSUPPORTED",
            "fixture has no connection",
        ))
    }

    async fn definitions(&self, _request_json: &str) -> Result<String, SdkDriverError> {
        Ok(self.catalog.to_string())
    }

    async fn preview_point_key(&self, request_json: &str) -> Result<String, SdkDriverError> {
        // 回显请求 JSON：证明 request_json / result_json 都是**逐字透传**
        //（SDK 与 Core 不解释、不重排、不裁剪）
        Ok(format!(r#"{{"echo":{request_json}}}"#))
    }
}

/// 测试夹具：实现了 RPC 但**业务拒绝**（用于 V5 的 kind 确定性断言）。
struct RejectingDefinitionsDriver;

#[async_trait::async_trait]
impl Driver for RejectingDefinitionsDriver {
    fn metadata(&self) -> mesa_core_types::DriverMetadata {
        mesa_core_types::DriverMetadata {
            driver_id: "definitions-rejecting".into(),
            name: "Definitions Rejecting".into(),
            version: "0.0.0".into(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        _config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        Err(SdkDriverError::new(
            ErrorKind::Unsupported,
            "UNSUPPORTED",
            "fixture has no connection",
        ))
    }

    async fn definitions(&self, _request_json: &str) -> Result<String, SdkDriverError> {
        Err(SdkDriverError::new(
            ErrorKind::Configuration,
            "INVALID_QUERY",
            "fixture rejects every query",
        ))
    }
}

/// 测试夹具：**记录**收到的 `request_json`（用于「逐字保留」用例）。
///
/// `log` 同时记录 `open_connection` 是否被调用：一条日志即可同时证明
/// 「零 Connection 条目」与「请求 JSON 逐字到达」两件事，不需要额外状态。
struct VerbatimDefinitionsDriver {
    log: Arc<Mutex<Vec<String>>>,
    catalog: &'static str,
}

#[async_trait::async_trait]
impl Driver for VerbatimDefinitionsDriver {
    fn metadata(&self) -> mesa_core_types::DriverMetadata {
        mesa_core_types::DriverMetadata {
            driver_id: "definitions-verbatim".into(),
            name: "Definitions Verbatim".into(),
            version: "0.0.0".into(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        _config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        // 本用例要求**零 Connection 条目**：真被调用就记下来，让断言失败
        self.log.lock().unwrap().push("OpenConnection".into());
        Err(SdkDriverError::new(
            ErrorKind::Unsupported,
            "UNSUPPORTED",
            "fixture has no connection",
        ))
    }

    async fn definitions(&self, request_json: &str) -> Result<String, SdkDriverError> {
        // 原样记录：断言「逐字相等」用的就是这份记录，SDK 任何重排/裁剪都会暴露
        self.log.lock().unwrap().push(request_json.to_string());
        Ok(self.catalog.to_string())
    }
}

// ---------------------------------------------------------------------------
// I28：协商语义不变（取 min）＋ 既有 4 个 Minor 门行为不变
// ---------------------------------------------------------------------------

/// I28（DC §8.3）：新 Core ↔ 旧 Driver（minor 6）⇒ 协商 6；既有 4 个 Minor
/// 门（2/3/5/6，全部 `>=` 语义）行为不变；新增的 I-2 门在 6 上必须关闭。
#[tokio::test]
async fn negotiated_minor_semantics_unchanged() {
    let (port, log, cancel) = start_legacy_minor6_driver().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN)
        .await
        .expect("旧驱动握手必须成功（Major 相同）");

    // ① 协商取双方较小值：min(7, 6) = 6
    assert_eq!(
        session.negotiated_minor(),
        6,
        "新 Core ↔ 旧驱动必须协商到 6"
    );
    // ② Welcome 如实回协商值（不是 Core 自身 Minor）——旧驱动据此知道自己被
    //    当作 1.6 对待。替身在自己的任务里记录，故先等它读到 Welcome。
    wait_until(2, || {
        log.lock().unwrap().iter().any(|n| n == "Welcome(minor=6)")
    })
    .await;
    assert!(
        log.lock().unwrap().iter().any(|n| n == "Welcome(minor=6)"),
        "Welcome 必须回 accepted_minor=6，实测收到: {:?}",
        log.lock().unwrap()
    );

    // ③ 既有 4 个 Minor 门在协商值 6 上全部放行（`>=` 语义未变）
    let negotiated = session.negotiated_minor();
    assert!(negotiated >= mesa_driver_protocol::PROBE_RPC_MIN_MINOR);
    assert!(negotiated >= mesa_driver_protocol::EVENT_PLANE_MIN_MINOR);
    assert!(negotiated >= mesa_driver_protocol::TASK_SCHEDULE_MIN_MINOR);
    assert!(negotiated >= mesa_driver_protocol::CONTROL_MODEL_MIN_MINOR);

    // ④ 新增的 I-2 门在 6 上必须关闭
    assert_eq!(
        DEFINITIONS_RPC_MIN_MINOR, PROTOCOL_MINOR,
        "门控常量必须等于引入该 RPC 的 Minor"
    );
    assert!(
        !definitions_query_supported(negotiated),
        "negotiated_minor=6 时 I-2 RPC 必须判为不支持"
    );
    assert!(definitions_query_supported(PROTOCOL_MINOR));

    // ⑤ 既有消息集在本会话上照常工作（Ping 往返）
    let pong = session
        .call(pb::envelope::Body::Ping(pb::Ping {}))
        .await
        .expect("Ping 必须有应答");
    assert!(
        matches!(pong.body, Some(pb::envelope::Body::Pong(_))),
        "旧消息集必须照常工作，实测 {pong:?}"
    );

    teardown(&mut session, Some(cancel));
}

// ---------------------------------------------------------------------------
// I10：旧二进制必须先门控再发帧（V2）
// ---------------------------------------------------------------------------

/// I10（DC §8.3）：`negotiated_minor < 7` ⇒ **不发帧**，直接判不支持。
///
/// 本条同时给出「为什么必须先门控」的可执行反证：绕过门控直接发帧时，旧驱动
/// 会静默忽略（替身逐字复刻旧 SDK 的兜底分支），调用方只能等到超时。
#[tokio::test]
async fn definitions_stale_binary_is_gated_before_send() {
    let (port, log, cancel) = start_legacy_minor6_driver().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN)
        .await
        .expect("旧驱动握手必须成功");
    assert_eq!(session.negotiated_minor(), 6);

    // ① 门控不通过 ⇒ 调用方**连一帧都不发**（这就是「先门控再发帧」的语义）
    assert!(!definitions_query_supported(session.negotiated_minor()));
    // 先等替身读到 Welcome：否则「日志为空」会让下面的"没发帧"变成空断言
    wait_until(2, || {
        log.lock().unwrap().iter().any(|n| n == "Welcome(minor=6)")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seen = log.lock().unwrap().clone();
    assert!(
        !seen
            .iter()
            .any(|n| n == "GetDefinitions" || n == "PreviewPointKey"),
        "negotiated_minor < 7 时不得发送 tag 45–48，实测旧驱动收到: {seen:?}"
    );

    // ② 反证：绕过门控发帧 ⇒ 旧驱动静默忽略 ⇒ 调用方白等一个超时。
    //    这里用 300ms 的显式预算（生产上会是 5s 的 RPC 预算），语义一致。
    let started = std::time::Instant::now();
    let err = session
        .call_with_timeout(
            pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
                request_json: r#"{"family":"828d"}"#.into(),
            }),
            Duration::from_millis(300),
        )
        .await
        .expect_err("旧驱动不会应答 GetDefinitions");
    assert!(
        matches!(err, SessionError::Timeout),
        "绕过门控的后果必须是超时（不是某个伪错误），实测 {err}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    // ③ 帧确实到达了旧驱动（只是被忽略）——证明风险是真实的
    wait_until(2, || {
        log.lock().unwrap().iter().any(|n| n == "GetDefinitions")
    })
    .await;
    let seen = log.lock().unwrap().clone();
    assert!(
        seen.iter().any(|n| n == "GetDefinitions"),
        "帧应已到达旧驱动（只是被兜底分支忽略），实测: {seen:?}"
    );

    teardown(&mut session, Some(cancel));
}

// ---------------------------------------------------------------------------
// V1 / V5：SDK 默认实现必须快速返回 Unsupported（不是超时、不是「设备离线」）
// ---------------------------------------------------------------------------

/// V1 + V5（DC §7.7）：未实现 I-2 的驱动（此处用 test-driver，源码零改动）
/// 被调用时必须**快速**得到 `Unsupported` + `DEFINITIONS_UNSUPPORTED`，
/// 且 `kind` **不得**是 `ConnectionError` / `DeviceError`（那是「设备离线」语义，
/// 会把运维引向查机床）。
///
/// 与 I9 的关系：I9 的 HTTP 501 断言属阶段 2（REST 侧）；本条是同一判据在
/// **wire 层**的承担者，直接断言 `error.kind`。
#[tokio::test]
async fn definitions_default_impl_is_fast_unsupported_not_offline() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    // 刻意不发 OpenConnection：两条 RPC 都必须在**零 Connection 条目**下应答
    let started = std::time::Instant::now();
    let reply = session
        .call(pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
            request_json: r#"{"family":"828d"}"#.into(),
        }))
        .await
        .expect("GetDefinitions 必须有应答（不得超时）");
    let elapsed = started.elapsed();
    let report = match reply.body {
        Some(pb::envelope::Body::DefinitionsReport(r)) => r,
        other => panic!("期望 DefinitionsReport，实际 {other:?}"),
    };
    let result = report.result.expect("result 必填");
    assert!(!result.ok, "未实现 I-2 的驱动必须报失败");
    let err = result.error.expect("失败必须带 error_detail");
    assert_eq!(
        err.kind,
        ErrorKind::Unsupported.as_str(),
        "协议不支持必须是 Unsupported"
    );
    assert_ne!(
        err.kind,
        ErrorKind::Connection.as_str(),
        "不得误报为设备离线"
    );
    assert_ne!(err.kind, ErrorKind::Device.as_str(), "不得误报为设备故障");
    assert_eq!(err.code, "DEFINITIONS_UNSUPPORTED");
    assert!(
        report.definitions_json.is_empty(),
        "失败时不得带半截载荷，实测 {:?}",
        report.definitions_json
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "默认实现必须快速失败（RPC 预算 5s），实测 {elapsed:?}"
    );

    // 预览同判据
    let started = std::time::Instant::now();
    let reply = session
        .call(pb::envelope::Body::PreviewPointKey(pb::PreviewPointKey {
            request_json: r#"{"family":"828d"}"#.into(),
        }))
        .await
        .expect("PreviewPointKey 必须有应答（不得超时）");
    let elapsed = started.elapsed();
    let report = match reply.body {
        Some(pb::envelope::Body::PreviewIdentityReport(r)) => r,
        other => panic!("期望 PreviewIdentityReport，实际 {other:?}"),
    };
    let err = report
        .result
        .expect("result 必填")
        .error
        .expect("必须带 error");
    assert_eq!(err.kind, ErrorKind::Unsupported.as_str());
    assert_ne!(err.kind, ErrorKind::Connection.as_str());
    assert_ne!(err.kind, ErrorKind::Device.as_str());
    assert_eq!(err.code, "DEFINITIONS_UNSUPPORTED");
    assert!(report.result_json.is_empty(), "失败时不得返回残缺身份");
    assert!(elapsed < Duration::from_secs(1), "实测 {elapsed:?}");

    teardown(&mut session, Some(cancel));
}

// ---------------------------------------------------------------------------
// V6：正常路径（新 Core ↔ 新驱动）—— JSON 逐字透传 + 错误 kind 原样保留
// ---------------------------------------------------------------------------

/// V6（DC §7.7）：实现了 I-2 的驱动，两条 RPC 正常应答；目录 JSON 与预览
/// JSON 均**逐字透传**（不重排、不裁剪、不改编码），且驱动返回的 `ErrorKind`
/// 原样过线（V5 的确定性归属在实现方路径上同样成立）。
#[tokio::test]
async fn definitions_rpc_passes_json_verbatim_and_preserves_error_kind() {
    // 目录 JSON 故意带缩进/中文/键序，任何"顺手重新序列化"都会破坏逐字相等
    const CATALOG: &str = "{\n  \"definitions\": [{\"variable_id\":\"channel.r_parameter\",\"label\":\"通道 R 参数\"}],\n  \"note\": \"\"\n}";
    let (port, cancel) = start_driver_server(DefinitionsDriver { catalog: CATALOG }).await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    let reply = session
        .call(pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
            request_json: r#"{"family":"828d"}"#.into(),
        }))
        .await
        .expect("GetDefinitions 必须有应答");
    let report = match reply.body {
        Some(pb::envelope::Body::DefinitionsReport(r)) => r,
        other => panic!("期望 DefinitionsReport，实际 {other:?}"),
    };
    assert!(report.result.expect("result 必填").ok);
    assert_eq!(
        report.definitions_json, CATALOG,
        "目录 JSON 必须逐字透传（含缩进/中文/键序）"
    );

    // 预览：请求 JSON 与结果 JSON 都必须逐字往返
    let request = r#"{"family":"828d","kind":"nck_variable","target":{"channel":1},"parameters":{"index":5}}"#;
    let reply = session
        .call(pb::envelope::Body::PreviewPointKey(pb::PreviewPointKey {
            request_json: request.into(),
        }))
        .await
        .expect("PreviewPointKey 必须有应答");
    let report = match reply.body {
        Some(pb::envelope::Body::PreviewIdentityReport(r)) => r,
        other => panic!("期望 PreviewIdentityReport，实际 {other:?}"),
    };
    assert!(report.result.expect("result 必填").ok);
    assert_eq!(
        report.result_json,
        format!(r#"{{"echo":{request}}}"#),
        "request_json 必须逐字到达驱动、result_json 必须逐字回到 Core"
    );

    teardown(&mut session, Some(cancel));

    // 实现方的业务错误：kind 必须原样过线（Configuration 不得被改写成 Internal）
    let (port, cancel) = start_driver_server(RejectingDefinitionsDriver).await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    let reply = session
        .call(pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
            request_json: r#"{"family":"828d"}"#.into(),
        }))
        .await
        .expect("GetDefinitions 必须有应答");
    let report = match reply.body {
        Some(pb::envelope::Body::DefinitionsReport(r)) => r,
        other => panic!("期望 DefinitionsReport，实际 {other:?}"),
    };
    let err = report
        .result
        .expect("result 必填")
        .error
        .expect("必须带 error");
    assert_eq!(err.kind, ErrorKind::Configuration.as_str());
    assert_eq!(err.code, "INVALID_QUERY");
    teardown(&mut session, Some(cancel));
}

// ---------------------------------------------------------------------------
// I27b（契约补丁）：请求参数经 SDK 分发**逐字保留**
// ---------------------------------------------------------------------------

/// I27b：`GetDefinitions.request_json` 必须**逐字**经 SDK 分发到达驱动，
/// 驱动的 `definitions_json` 必须**逐字**回到调用方，且全程**零 Connection 条目**。
///
/// 为什么需要这条用例：目录的查询参数（family / variable_id / revision / limit /
/// cursor）与 `UNSUPPORTED_FAMILY` / `UNKNOWN_VARIABLE` 等错误的归属都在 Driver
/// 层，所以「参数原样过线」是本契约的唯一可行前提。请求/响应都故意带中文、缩进、
/// 引号/反斜杠/制表符：任何「顺手 serde 一下再序列化」都会让逐字相等失败。
///
/// 零连接由夹具自行记录（`open_connection` 被调用会写日志），因此断言不是
/// 「我们没发 OpenConnection」这种自述，而是**驱动侧从未收到过**连接请求。
#[tokio::test]
async fn definitions_request_json_survives_sdk_dispatch_verbatim() {
    // 含中文 / 换行缩进 / 转义引号 / 反斜杠 / 制表符的查询 JSON
    const QUERY: &str = "{\n  \"family\": \"828d\",\n  \"variable_id\": \"通道参数/R参数\",\n  \"note\": \"引号\\\"反斜杠\\\\制表\\t\",\n  \"limit\": 50,\n  \"cursor\": null\n}";
    // 响应同样带中文 / 缩进 / 特殊字符，防止「重新序列化」蒙混过关
    const CATALOG: &str = "{\n  \"definitions\": [{\"variable_id\":\"channel.r_parameter\",\"label\":\"通道 R 参数\",\"digest\":\"a\\\\b\"}],\n  \"next_cursor\": \"\"\n}";

    let log = Arc::new(Mutex::new(Vec::new()));
    let (port, cancel) = start_driver_server(VerbatimDefinitionsDriver {
        log: log.clone(),
        catalog: CATALOG,
    })
    .await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    // 刻意不发 OpenConnection：目录是驱动能力查询，零 Connection 条目也必须应答
    let reply = session
        .call(pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
            request_json: QUERY.into(),
        }))
        .await
        .expect("GetDefinitions 必须有应答（不得超时）");
    let report = match reply.body {
        Some(pb::envelope::Body::DefinitionsReport(r)) => r,
        other => panic!("期望 DefinitionsReport，实际 {other:?}"),
    };
    assert!(report.result.expect("result 必填").ok);
    assert_eq!(
        report.definitions_json, CATALOG,
        "definitions_json 必须逐字回到调用方（不重排/不裁剪/不改编码）"
    );

    // 驱动侧只应看到一次 definitions 调用，且收到的字符串与发送的**逐字相等**
    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        1,
        "全程只应有 1 次驱动侧调用（零 Connection 条目），实测 {seen:?}"
    );
    assert_eq!(
        seen[0], QUERY,
        "request_json 必须逐字到达驱动（中文/缩进/特殊字符不得被改动）"
    );
    assert!(
        !seen.iter().any(|n| n == "OpenConnection"),
        "不得创建任何 Connection 条目，实测 {seen:?}"
    );

    teardown(&mut session, Some(cancel));
}

// ---------------------------------------------------------------------------
// I34：旧 Manager ↔ 新驱动 ⇒ 静默降级（V3）
// ---------------------------------------------------------------------------

/// I34（DC §8.3）：旧 Manager（Minor 6）对新驱动二进制。
///
/// ① 协商结果 `accepted_protocol_minor == 6`；② 旧 Manager 永不发 tag 45–48；
/// ③ 新驱动在**旧消息集**上行为与本改动前逐字一致（无告警、无错误、无行为变化）。
///
/// NOTE: ③ 的「旧 Manager 对本端点无路由 ⇒ 404」属阶段 2 的 Core 路由事实
///（本阶段尚未新增路由），因此这里断言的是**wire 层**的降级安全性：
/// 用旧消息集走完整往返，且过程中不出现任何错误帧。
#[tokio::test]
async fn legacy_manager_new_driver_downgrade_is_silent() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let server = tokio::spawn(async move {
        mesa_driver_sdk::serve(mesa_test_driver::TestDriver, listener, TOKEN.into(), c).await
    });

    // 旧 Manager 的 Welcome：accepted_minor = 6（旧 Core 的 PROTOCOL_MINOR）
    let (mut rd, mut wr) = raw_core_handshake(port, 6).await;

    // ② 旧消息集：Ping 往返（行为与本改动前一致）
    let ping = pb::Envelope {
        msg_id: 100,
        body: Some(pb::envelope::Body::Ping(pb::Ping {})),
    };
    write_envelope(&mut wr, &ping).await.unwrap();
    let pong = read_envelope(&mut rd).await.expect("Ping 必须有应答");
    assert_eq!(pong.msg_id, 100, "响应必须复用 msg_id");
    assert!(
        matches!(pong.body, Some(pb::envelope::Body::Pong(_))),
        "降级会话上 Ping 必须正常，实测 {pong:?}"
    );

    // ② 旧消息集：GetDescriptor 往返（管理面在旧消息集上照常可用）
    let get = pb::Envelope {
        msg_id: 101,
        body: Some(pb::envelope::Body::GetDescriptor(pb::GetDescriptor {})),
    };
    write_envelope(&mut wr, &get).await.unwrap();
    let report = read_envelope(&mut rd)
        .await
        .expect("GetDescriptor 必须有应答");
    match report.body {
        Some(pb::envelope::Body::DescriptorReport(d)) => {
            // Descriptor 契约 2/0 不变（I30 的 wire 侧旁证）
            assert_eq!(d.contract_major, 2);
            assert_eq!(d.contract_minor, 0);
            assert!(!d.descriptor_json.is_empty());
        }
        other => panic!("期望 DescriptorReport，实际 {other:?}"),
    }

    // ③ 全过程无错误帧、无告警、无行为变化：驱动仍在服务（再 Ping 一次）
    let ping = pb::Envelope {
        msg_id: 102,
        body: Some(pb::envelope::Body::Ping(pb::Ping {})),
    };
    write_envelope(&mut wr, &ping).await.unwrap();
    let pong = read_envelope(&mut rd).await.expect("降级后仍必须可服务");
    assert!(
        matches!(pong.body, Some(pb::envelope::Body::Pong(_))),
        "降级是静默的：不得出现错误帧，实测 {pong:?}"
    );
    assert!(!server.is_finished(), "新驱动不得因旧 Manager 而降级失败");

    drop(wr);
    cancel.cancel();
}

// ---------------------------------------------------------------------------
// I35：未知消息类型不得导致驱动进程异常退出（V4）
// ---------------------------------------------------------------------------

/// 构造一个「信封层无法识别」的帧：`msg_id = 1` + 未知字段号 99（wire type 2，
/// 空 payload）。prost 会丢弃未知字段 ⇒ 解码后 `body == None`，正好落进
/// SDK 的兜底分支（`other => tracing::debug!(...)`）。
fn unknown_variant_frame() -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    // field 1 (msg_id), varint 1
    body.extend_from_slice(&[0x08, 0x01]);
    // field 99, wire type 2：key = 99 << 3 | 2 = 794 = varint [0x9A, 0x06]
    body.extend_from_slice(&[0x9A, 0x06, 0x00]);
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// I35（DC §8.3）：注入未知 oneof 变体后，驱动 ① 不 panic、不退出；
/// ② 连接不中断（后续 `Ping` / `GetDescriptor` 正常）；③ 既有兜底分支行为保持。
///
/// 除「未知 tag」外还注入一个**方向错误但已知**的变体（`Pong` 由 Core 发往
/// 驱动），它同样落进兜底分支（`Some(other)` 而非 `None`），两条路径都不得
/// 导致连接中断。
#[tokio::test]
async fn unknown_envelope_variant_never_aborts_driver_process() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let server = tokio::spawn(async move {
        mesa_driver_sdk::serve(mesa_test_driver::TestDriver, listener, TOKEN.into(), c).await
    });

    let (mut rd, mut wr) = raw_core_handshake(port, PROTOCOL_MINOR).await;

    // ① 未知 tag（信封层解不出来 ⇒ body == None）
    wr.write_all(&unknown_variant_frame()).await.unwrap();
    wr.flush().await.unwrap();

    // ① 方向错误但已知的变体（body == Some(Pong)）
    let stray = pb::Envelope {
        msg_id: 200,
        body: Some(pb::envelope::Body::Pong(pb::Pong {})),
    };
    write_envelope(&mut wr, &stray).await.unwrap();

    // ② 连接不中断：后续 RPC 必须正常
    let ping = pb::Envelope {
        msg_id: 201,
        body: Some(pb::envelope::Body::Ping(pb::Ping {})),
    };
    write_envelope(&mut wr, &ping).await.unwrap();
    let pong = read_envelope(&mut rd)
        .await
        .expect("注入未知消息后 Ping 仍必须应答");
    assert_eq!(pong.msg_id, 201);
    assert!(matches!(pong.body, Some(pb::envelope::Body::Pong(_))));

    let get = pb::Envelope {
        msg_id: 202,
        body: Some(pb::envelope::Body::GetDescriptor(pb::GetDescriptor {})),
    };
    write_envelope(&mut wr, &get).await.unwrap();
    let report = read_envelope(&mut rd)
        .await
        .expect("注入未知消息后 GetDescriptor 仍必须应答");
    assert!(
        matches!(report.body, Some(pb::envelope::Body::DescriptorReport(_))),
        "未知消息不得影响后续管理面 RPC，实测 {report:?}"
    );

    // ① 进程（服务任务）存活
    assert!(
        !server.is_finished(),
        "未知消息类型不得导致驱动退出（兜底分支只记 debug 日志）"
    );

    drop(wr);
    cancel.cancel();
}

// ---------------------------------------------------------------------------
// 非 NCK 驱动兼容（I30 的源码零改动证明）
// ---------------------------------------------------------------------------

/// 非 NCK 驱动兼容：S7 / FOCAS2 / OPC UA / test-driver **源码零改动**即可编译
/// 并通过，且行为不变。
///
/// 证据分两层：
/// - **编译期**：本测试 crate 依赖这四个驱动 crate；若它们因新增 trait 方法而
///   需要改源码，这里根本编译不过（本 PR 只改了它们的 `driver.toml` 的 minor）。
/// - **运行期**：它们都继承 trait 的**默认实现** ⇒ `Unsupported` /
///   `DEFINITIONS_UNSUPPORTED`；Descriptor 契约仍 2/0（行为不变）。
#[tokio::test]
async fn non_nck_drivers_inherit_unsupported_default() {
    let drivers: Vec<(&str, Box<dyn Driver>)> = vec![
        // 三个驱动都是 unit struct（`#[derive(Default)]`），直接构造即可
        ("s7", Box::new(mesa_driver_s7::S7Driver)),
        ("focas2", Box::new(mesa_driver_focas2::FocasDriver)),
        ("opcua", Box::new(mesa_driver_opcua::OpcUaDriver)),
        ("test-driver", Box::new(mesa_test_driver::TestDriver)),
    ];
    for (expected_id, driver) in drivers {
        assert_eq!(
            driver.metadata().driver_id,
            expected_id,
            "{expected_id} 的身份不得变化"
        );
        // Descriptor 契约 2/0 不变（行为不变的旁证）
        let d = driver.descriptor();
        assert_eq!(
            (d.contract_major, d.contract_minor),
            (2, 0),
            "{expected_id} 的 Descriptor 契约必须仍是 2/0"
        );
        // 两条 I-2 RPC 均走默认实现：快速 Unsupported
        let err = driver
            .definitions(r#"{"family":"828d"}"#)
            .await
            .expect_err("{expected_id} 不应实现 I-2 目录");
        assert_eq!(err.kind, ErrorKind::Unsupported, "{expected_id}");
        assert_eq!(err.code, "DEFINITIONS_UNSUPPORTED", "{expected_id}");
        assert_ne!(err.kind, ErrorKind::Connection, "{expected_id}");
        assert_ne!(err.kind, ErrorKind::Device, "{expected_id}");
        let err = driver
            .preview_point_key(r#"{"family":"828d"}"#)
            .await
            .expect_err("{expected_id} 不应实现 I-2 预览");
        assert_eq!(err.kind, ErrorKind::Unsupported, "{expected_id}");
        assert_eq!(err.code, "DEFINITIONS_UNSUPPORTED", "{expected_id}");
    }
}
