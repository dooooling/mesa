//! 逐 block 的 golden 线缆字节测试（Workstream A：原生地址证据 → 可执行断言）。
//!
//! 为什么需要这个文件
//! ----------------
//! `docs/evidence/nck-native-address-evidence.json` 的 `block_observations` 记录了
//! 五个 block 在**一台实体 828D** 上被接受过的请求字节（S/RP/SSP/SGA/SEGA）。
//! 那些字节只有转录文本，没有回归保护：一旦 `codec`/`native` 的编码路径漂移
//! （字节序、areaunit 位运算、module 位置），证据就与实现脱钩而没人发现。
//! 本文件把每条观测**逐字节**钉成期望值。
//!
//! 期望值从哪来（不得从手册推断 module）
//! ----------------------------------
//! - 10 字节结构（`0x12 0x08 <syntax> <areaunit> <column:u16BE> <line:u16BE>
//!   <module:u8> <linecount:u8>`）来自协议解码器 Wireshark
//!   `packet-s7comm.c`（`s7comm_decode_param_item` / `s7comm_syntaxid_nck`），
//!   对应 `src/codec.rs`；
//! - 每条的 `module` 字节**只**来自 `block_observations[i].module_decimal`
//!   （真机观测），每个期望字节串后面都标注了它在
//!   `docs/evidence/nck-live-samples-probe.txt` 的行号；
//! - 手册（828D NC variables 03/2025）只提供语义与列索引，**不提供** module
//!   字节，因此本文件里没有任何一个 module 值来自手册推断。
//!
//! 边界
//! ----
//! - 不连机床、不发请求：全部是纯计算（`NativeNckAddress → resolve_native →
//!   encode_var_spec`）。
//! - 只覆盖**已有真机观测**的 block；`SPARPP`(122)/`SPARP`(125) 仍属未解决，
//!   见文件末尾的 `unobserved_blocks_must_not_get_golden_bytes`——它们只允许
//!   标注「未解决」，不许编造期望字节。

use mesa_driver_sinumerik_nck::{
    NativeNckAddress, NckDataKind, PresetRegistry, encode_var_spec, resolve_native,
};
use serde_json::{Map, Value, json};

/// 证据矩阵（仓内相对路径；测试直接读仓库文件，防止矩阵与测试各说各话）。
const EVIDENCE_JSON: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/evidence/nck-native-address-evidence.json"
);

/// 一条 golden 用例：输入是用户在 Resource 里给出的原生地址字段，
/// 期望是该地址编码出的**完整 10 字节**请求规范。
struct Golden {
    /// block 缩写（S/RP/SSP/SGA/SEGA），与证据矩阵 `block_observations[i].block` 对应。
    block: &'static str,
    variable: &'static str,
    area: &'static str,
    area_no: u8,
    module: u8,
    column: u16,
    line: u16,
    count: u8,
    decode: &'static str,
    transport: &'static str,
    /// 期望字节（空格分隔的十六进制，便于与转录文本逐字对照）。
    expected: &'static str,
    /// 观测出处：`docs/evidence/nck-live-samples-probe.txt` 的行号。
    sample_line: u32,
    /// 该行设备的返回，用于区分「可读」与「仅编码一致」。
    response: &'static str,
}

/// 五个已观测 block 的全部观测行（24 条，与证据矩阵的 5 条主行 +
/// 19 条附加观测一一对应）。
const GOLDEN: &[Golden] = &[
    // ---- S（module 127 = 0x7f）：C/S 的 progStatus 与 actFeedRateIpo ----
    Golden {
        block: "S",
        variable: "channel.program.status (progStatus)",
        area: "C",
        area_no: 1,
        module: 127,
        column: 13,
        line: 1,
        count: 1,
        decode: "U16_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 0d 00 01 7f 01",
        sample_line: 8,
        response: "GOOD value=U32(3)",
    },
    Golden {
        block: "S",
        variable: "channel.feed.actual (actFeedRateIpo)",
        area: "C",
        area_no: 1,
        module: 127,
        column: 1,
        line: 1,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 01 00 01 7f 01",
        sample_line: 9,
        response: "GOOD value=F64(0.0)",
    },
    // ---- RP（module 21 = 0x15）：line = R 号 + 1 ----
    Golden {
        block: "RP",
        variable: "channel.r_parameter (R0)",
        area: "C",
        area_no: 1,
        module: 21,
        column: 1,
        line: 1,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 01 00 01 15 01",
        sample_line: 11,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "RP",
        variable: "channel.r_parameter (R1)",
        area: "C",
        area_no: 1,
        module: 21,
        column: 1,
        line: 2,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 01 00 02 15 01",
        sample_line: 12,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "RP",
        variable: "channel.r_parameter (R99)",
        area: "C",
        area_no: 1,
        module: 21,
        column: 1,
        line: 100,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 01 00 64 15 01",
        sample_line: 13,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        // 设备回 ITEM_REJECTED 0x03：这条只证明**编码**（line=426=0x01aa 拆成两字节），
        // 不证明该 line 在设备上可读，因此不得用来宣称 RP 的读范围。
        block: "RP",
        variable: "channel.r_parameter (R425，设备拒绝)",
        area: "C",
        area_no: 1,
        module: 21,
        column: 1,
        line: 426,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 01 01 aa 15 01",
        sample_line: 14,
        response: "ITEM_REJECTED 0x03",
    },
    // ---- SSP（module 114 = 0x72）：line = 主轴索引 ----
    Golden {
        block: "SSP",
        variable: "spindle.speed.commanded (cmdSpeed, spindle 1)",
        area: "C",
        area_no: 1,
        module: 114,
        column: 3,
        line: 1,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 03 00 01 72 01",
        sample_line: 34,
        response: "GOOD value=F64(10000.0)",
    },
    Golden {
        block: "SSP",
        variable: "spindle.speed.commanded (cmdSpeed, spindle 2)",
        area: "C",
        area_no: 1,
        module: 114,
        column: 3,
        line: 2,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 03 00 02 72 01",
        sample_line: 35,
        response: "GOOD value=F64(0.0)",
    },
    // ---- SGA（module 115 = 0x73）：设定位置 MCS，line = 机床轴号 1..8 ----
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 1)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 1,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 01 73 01",
        sample_line: 16,
        response: "GOOD value=F64(-1.9672...)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 2)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 2,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 02 73 01",
        sample_line: 17,
        response: "GOOD value=F64(-3.5154...)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 3)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 3,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 03 73 01",
        sample_line: 18,
        response: "GOOD value=F64(25.5511...)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 4)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 4,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 04 73 01",
        sample_line: 19,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 5)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 5,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 05 73 01",
        sample_line: 20,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 6)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 6,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 06 73 01",
        sample_line: 21,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 7)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 7,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 07 73 01",
        sample_line: 22,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SGA",
        variable: "axis.position.commanded_mcs (actToolBasePos, axis 8)",
        area: "C",
        area_no: 1,
        module: 115,
        column: 2,
        line: 8,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 02 00 08 73 01",
        sample_line: 23,
        response: "GOOD value=F64(0.0)",
    },
    // ---- SEGA（module 112 = 0x70）：实际位置 BCS，column 33 = 0x21 ----
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 1)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 1,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 01 70 01",
        sample_line: 25,
        response: "GOOD value=F64(-1.9672...)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 2)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 2,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 02 70 01",
        sample_line: 26,
        response: "GOOD value=F64(-3.5154...)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 3)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 3,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 03 70 01",
        sample_line: 27,
        response: "GOOD value=F64(25.5511...)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 4)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 4,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 04 70 01",
        sample_line: 28,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 5)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 5,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 05 70 01",
        sample_line: 29,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 6)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 6,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 06 70 01",
        sample_line: 30,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 7)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 7,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 07 70 01",
        sample_line: 31,
        response: "GOOD value=F64(0.0)",
    },
    Golden {
        block: "SEGA",
        variable: "axis.position.actual_bcs (actProgPosBKS, axis 8)",
        area: "C",
        area_no: 1,
        module: 112,
        column: 33,
        line: 8,
        count: 1,
        decode: "F64_LE",
        transport: "0x09",
        expected: "12 08 82 41 00 21 00 08 70 01",
        sample_line: 32,
        response: "GOOD value=F64(0.0)",
    },
];

/// 由 golden 用例构造「用户显式给出的原生地址字段」。
fn params(g: &Golden) -> Map<String, Value> {
    let v = json!({
        "area": g.area,
        "area_no": g.area_no,
        "module": g.module,
        "column": g.column,
        "line": g.line,
        "count": g.count,
        "unit_mode": "current",
        "decode": g.decode,
        "transport_size": g.transport,
    });
    v.as_object().unwrap().clone()
}

fn hex_of(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn load_evidence() -> Value {
    let raw = std::fs::read_to_string(EVIDENCE_JSON)
        .unwrap_or_else(|e| panic!("读不到证据矩阵 {EVIDENCE_JSON}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("证据矩阵不是合法 JSON: {e}"))
}

/// 核心断言：给定 (area, module, column, line, area_no)，编码必须逐字节等于观测值。
///
/// 10 个字节各自的来源（以 S 行 `12 08 82 41 00 0d 00 01 7f 01` 为例）：
/// - `0x12` 规范类型、`0x08` 规范体长 8 —— 协议分发条件（codec 常量）；
/// - `0x82` current 单位语法（探针全部用当前单位读，无 0x83/0x84 观测）；
/// - `0x41` = area C(2) << 5 | area_no 1 —— 位运算来自协议布局；
/// - `00 0d` column=13 大端、`00 01` line=1 大端；
/// - `7f` module=127 —— 只来自真机观测（S 行），手册不给这个字节；
/// - `01` line_count=count=1。
#[test]
fn confirmed_block_request_bytes_are_exact() {
    for g in GOLDEN {
        let addr = NativeNckAddress::from_parameters(&params(g))
            .unwrap_or_else(|e| panic!("{} {} 地址应合法: {e}", g.block, g.variable));
        let rp = resolve_native(addr)
            .unwrap_or_else(|e| panic!("{} {} resolve 失败: {e}", g.block, g.variable));
        let actual = encode_var_spec(&rp.wire);
        assert_eq!(
            hex_of(&actual),
            g.expected,
            "{} {}（观测于 nck-live-samples-probe.txt:{}，设备返回 {}）",
            g.block,
            g.variable,
            g.sample_line,
            g.response
        );
        // 结构侧同步核对：改写 10 字节中任一字段都会让上面失败，这里再钉住结构化值。
        assert_eq!(
            rp.wire.syntax_id, 0x82,
            "{} 探针全部用 current 单位",
            g.block
        );
        assert_eq!(
            rp.wire.area_unit,
            (2 << 5) | g.area_no,
            "{} areaunit",
            g.block
        );
        assert_eq!(rp.wire.column, g.column, "{} column", g.block);
        assert_eq!(rp.wire.line, g.line, "{} line", g.block);
        assert_eq!(rp.wire.module, g.module, "{} module", g.block);
        assert_eq!(rp.wire.line_count, g.count, "{} linecount", g.block);
        assert_eq!(
            rp.kind,
            NckDataKind::parse_decode(g.decode).unwrap(),
            "{} decode 应随绑定",
            g.block
        );
    }
    // 覆盖度自检：五个已确认 block 每个至少一条，且总数与证据矩阵观测数一致。
    for block in ["S", "RP", "SSP", "SGA", "SEGA"] {
        assert!(
            GOLDEN.iter().any(|g| g.block == block),
            "block {block} 缺 golden 用例"
        );
    }
    assert_eq!(
        GOLDEN.len(),
        24,
        "观测行总数应为 24（5 主行 + 19 附加观测）"
    );
}

/// golden 表与证据矩阵必须互相印证：module 只能来自 `block_observations`。
///
/// 这条测试的意义在于「双向钉死」：矩阵改了 module 而测试没改（或反过来）都会
/// 立刻失败，从而不可能出现「测试期望值凭空产生」的情况。
#[test]
fn golden_table_agrees_with_evidence_matrix() {
    let doc = load_evidence();
    let rows = doc["block_observations"]
        .as_array()
        .expect("block_observations 必须是数组");
    for g in GOLDEN {
        let row = rows
            .iter()
            .find(|r| r["block"].as_str() == Some(g.block))
            .unwrap_or_else(|| panic!("证据矩阵没有 block {} 的观测行", g.block));
        assert_eq!(
            row["module_decimal"].as_u64(),
            Some(u64::from(g.module)),
            "{} 的 module 必须与矩阵观测一致（不得由手册或推测给出）",
            g.block
        );
        assert_eq!(
            row["module_value_source"]["evidence_class"].as_str(),
            Some("device_observation"),
            "{} 的 module 出处必须是真机观测类",
            g.block
        );
    }

    // 矩阵里每一条观测（主行 + 附加观测）都必须能由本文件同一编码路径复现。
    // 主行的字段取自 `device_observation.decoded`（它本身由 raw bytes 解出），
    // 附加观测自带字段；两者都不含任何手工推断的值。
    for row in rows {
        let block = row["block"].as_str().unwrap();
        let decoded = &row["device_observation"]["decoded"];
        let mut checks = vec![(decoded.clone(), "primary")];
        if let Some(add) = row["additional_observations"].as_array() {
            for obs in add {
                checks.push((obs.clone(), "additional"));
            }
        }
        for (obs, kind) in checks {
            let raw = if kind == "primary" {
                row["raw_request_example"].as_str().unwrap().to_string()
            } else {
                obs["raw_request"].as_str().unwrap().to_string()
            };
            // 用矩阵自己声明的字段拼地址：area/area_no/module/column/line/line_count。
            let p = json!({
                "area": obs["area"].as_str().unwrap_or("C"),
                "area_no": obs["area_no"].as_u64().unwrap_or(1),
                "module": obs["module"].as_u64().unwrap_or_else(|| {
                    decoded["module"].as_u64().unwrap()
                }),
                "column": obs["column"].as_u64().unwrap(),
                "line": obs["line"].as_u64().unwrap(),
                "count": obs["line_count"].as_u64().unwrap(),
                "unit_mode": "current",
                "decode": "F64_LE",
                "transport_size": "0x09",
            });
            let addr = NativeNckAddress::from_parameters(p.as_object().unwrap())
                .unwrap_or_else(|e| panic!("{block} {kind} 地址应合法: {e}"));
            let rp = resolve_native(addr).unwrap();
            assert_eq!(
                hex_of(&encode_var_spec(&rp.wire)),
                raw,
                "{block} {kind} 观测字节必须由同一编码路径复现"
            );
        }
    }
}

/// 未解决 block 的显式标注：`SPARPP`(122)/`SPARP`(125) **不得**拥有 golden 字节。
///
/// 这两个 module 值没有真机观测（只存在于候选模板），也未被 03/2025 手册确认为
/// native module。Workstream A 的规则是「无法证明就保持 unresolved」，所以这里
/// 用断言把「不许偷偷补一条期望值」变成可执行的约束：
/// - 证据矩阵不得出现 SPARPP/SPARP 的 `block_observations` 行；
/// - 候选模板必须仍然标 `validation_status = candidate`（不得升级为已验证）；
/// - 手册语义侧仍把 `channel.program.name.main` 标为待复核。
#[test]
fn unobserved_blocks_must_not_get_golden_bytes() {
    const UNRESOLVED_BLOCKS: [(&str, u8); 2] = [("SPARPP", 122), ("SPARP", 125)];

    // 1) golden 表里不许出现未解决的 module 值。
    for (block, module) in UNRESOLVED_BLOCKS {
        assert!(
            !GOLDEN.iter().any(|g| g.block == block),
            "block {block} 未解决，不得有 golden 字节"
        );
        assert!(
            !GOLDEN.iter().any(|g| g.module == module),
            "module {module}（{block}）未解决，不得出现在 golden 表"
        );
    }

    // 2) 证据矩阵必须继续把它们留作未解决。
    let doc = load_evidence();
    let rows = doc["block_observations"].as_array().unwrap();
    for (block, _) in UNRESOLVED_BLOCKS {
        assert!(
            !rows.iter().any(|r| r["block"].as_str() == Some(block)),
            "block {block} 没有真机观测，不得出现在 block_observations"
        );
    }
    let pending = doc["manual_mapping"]["result"]["pending_review"]
        .as_array()
        .unwrap();
    assert!(
        pending
            .iter()
            .any(|v| v.as_str() == Some("channel.program.name.main")),
        "channel.program.name.main 仍未解决，必须留在 pending_review"
    );
    let unresolved = doc["unresolved"].as_array().unwrap();
    assert!(
        unresolved
            .iter()
            .any(|u| u["id"].as_str() == Some("module-value-authority")),
        "module 值的权威来源仍未解决，必须留在 unresolved"
    );

    // 3) 候选模板不得被悄悄升级为已验证。
    let reg = PresetRegistry::embedded();
    for (block, module) in UNRESOLVED_BLOCKS {
        let prefix = format!("828d.C.{block}.");
        let t = reg
            .for_family("828d")
            .into_iter()
            .find(|t| {
                t.id.starts_with(&prefix)
                    && t.parameters.get("module").and_then(|v| v.as_u64())
                        == Some(u64::from(module))
            })
            .unwrap_or_else(|| panic!("缺候选模板 {prefix} module{module}"));
        assert_eq!(
            t.validation_status, "candidate",
            "{block} 无真机观测，模板必须仍为 candidate"
        );
    }
}
