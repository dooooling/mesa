//! N 区 `column=21`（`chanAssignment`）的**布局推导向量**，非设备证据。
//!
//! 证据标记：`layout-derived`（布局推导）。本文件所有向量都带这个标记，
//! 便于全局检索时与 `docs/evidence/` 下的真机证据（`device_observation`）区分。
//!
//! ## 本文件的证据等级（必读，不得抬升）
//!
//! 本文件为**布局推导向量**（`layout-derived`），非设备证据：全部期望字节都是按现有编码布局
//! （`src/codec.rs` 的 10 字节 item 布局 + `src/native.rs` 的 area_no→unit 合成规则）
//! 推导出来的，**没有**任何真机抓包与之对应，也**不构成**“机床会接受/会按此返回”
//! 的证据。它只回答一个问题：Mesa 的编码路径**能否表达** N 区的 `column=21`。
//!
//! 因此：
//! - 本文件**不写入** `docs/evidence/` 下任何 `.txt`/`.json`（那是真机证据集合）；
//! - 本文件**不进入** `tests/block_wire_golden.rs` 的已观测 block 表
//!   （该表每条 module 字节都来自实体 828D 观测，混入推导值会污染证据等级）；
//! - 文件末尾 `layout_derived_vector_is_absent_from_device_evidence` 把上述两条
//!   边界变成可执行断言，防止后续有人把推导值“升级”成证据。
//!
//! NOTE(真机证据已出现，边界改写而非放宽): 后续的一次性真机只读验证产出了
//! `docs/evidence/nck-live-chan-assignment-probe.txt`（`device-observed`），
//! 它记录了**同一个请求字节**在实体 828D 上的真实响应，结论是
//! “`module=26` 下返回的载荷与手册 `TYPE_UWORD` 不符 ⇒ 结果待核实”，
//! 即：它**没有**把本文件的推导值抬升成“已确认可读”，反而给出了否定证据。
//! 因此守卫改为：除该文件外，证据目录里任何文件仍不得提到 `chanAssignment`
//! 或本文件的字节串；而对该文件加**正面断言**——必须逐字记录设备返回字节、
//! 必须标注 `device-observed`、必须保留“待核实/UNDETERMINED”的结论，
//! 且不得出现任何具体的方式组分配结果（`assigned(<数字>`）。
//! 这样既保留了原意（推导不得冒充证据），也不会因为真机证据存在而失效。
//!
//! ## 为什么是 `column=21` 而不是 `column=10010`
//!
//! 手册 `docs/reference/manuals/828D_ncvar_lists_man_0325_en-US.pdf` p1065 的
//! `chanAssignment chanAssignment (N, Y)` 条目原文给出：
//! `MD10010: ASSIGN_CHAN_TO_MODE_GROUP[x]`（x = ChannelNo）、
//! `Return value 0 = channel does not exist / n = channel assigned to mode group n`、
//! `Data type: TYPE_UWORD`、`Column index: 21`、`Line index: Channel number`。
//!
//! 注意区分两个不同层次的东西：
//! - `Column index: 21` 是**OPI 选择器里的列号**，即本测试编码进 `[4..6]` 的值；
//! - `MD10010` 是该变量**反映的机床数据编号**，`10010 = 0x271A`，它是
//!   “另一个变量的列号形态”（N 区里 `column index >= 1000` 的 MD 号族，例如
//!   `axis.name` 用的 `10000`），**不是** `chanAssignment` 的列号。
//!
//! 用 `column=10010` 去读会命中**另一个变量**而不是 `chanAssignment`，所以本文件
//! 专门用反向断言钉死这一点（`column_21_is_not_the_md_number_10010_layout_derived`）。
//!
//! ## 边界
//!
//! 不连机床、不发请求、不改生产库：全部是纯计算
//! （`NckWireAddress` / `NativeNckAddress → resolve_native → encode_var_spec`）。
//! 本文件**不**实现 `chanAssignment` 探针（`identity_spec.rs` 的 `PROBES_828D`
//! 与 `identity.rs` 的枚举逻辑均未改动）。

use mesa_driver_sinumerik_nck::{
    CodecError, NativeNckAddress, NckArea, NckDataKind, NckWireAddress, encode_var_spec,
    resolve_native,
};
use serde_json::{Map, Value, json};

/// `chanAssignment` 的 10 字节请求规范（布局推导，非抓包）。
///
/// 逐字节来源：
/// - `0x12` 规范类型、`0x08` 规范体长 8 —— `codec` 常量（协议分发条件）；
/// - `0x82` current 单位语法（`NckUnitMode::Current`）；
/// - `0x01` = area N(0) << 5 | area_no 1 —— `NckWireAddress::area_unit` 位运算；
/// - `0x00 0x15` = column 21 大端 —— 手册 p1065 `Column index: 21`；
/// - `0x00 0x01` = line 1 大端 —— 手册 `Line index: Channel number`，通道 1；
/// - `0x1a` = module 26 —— **不是**手册值，沿用既有 `AXIS_NAME` 探针的 module 26
///   （同属 N 区，`identity_spec.rs`）；module 字节的权威来源仍是未解决项；
/// - `0x01` = line_count = count 1。
const CHAN_ASSIGNMENT_LAYOUT_DERIVED: [u8; 10] =
    [0x12, 0x08, 0x82, 0x01, 0x00, 0x15, 0x00, 0x01, 0x1a, 0x01];

/// `chanAssignment` 的 `Column index`（手册 p1065）——小索引族，不是 MD 号。
const CHAN_ASSIGNMENT_COLUMN: u16 = 21;
/// 该条目反映的机床数据编号 `MD10010`——**不是**列号，仅用于反向断言。
const CHAN_ASSIGNMENT_MD_NUMBER: u16 = 10010;

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 构造 `chanAssignment` 的原生地址字段（用户显式给出的协议字段形态）。
fn chan_assignment_params(column: u16, line: u16) -> Map<String, Value> {
    json!({
        "area": "N",
        "area_no": 1,
        "module": 26,
        "column": column,
        "line": line,
        "count": 1,
        "unit_mode": "current",
        "decode": "U16_LE",
        "transport_size": "0x09",
    })
    .as_object()
    .unwrap()
    .clone()
}

/// 用例 1：N/1 + column=21 + line=1 + module=26 + count=1 的字节必须逐字相等。
///
/// 两条路径同时断言，避免“结构体对但语义层拼错”：
/// ① 直接构造 `NckWireAddress`（纯结构 → 字节）；
/// ② 走 `NativeNckAddress::from_parameters → resolve_native`（用户字段 → 结构），
///    确认 area_no→unit 的合成规则不会把 N 区拼成 C 区。
#[test]
fn chan_assignment_exact_bytes_layout_derived() {
    // ① 结构化路径：area N(0) << 5 | 1 = 0x01。
    let area_unit = NckWireAddress::area_unit(NckArea::Nck.index(), 1).unwrap();
    let wire = NckWireAddress {
        syntax_id: 0x82,
        area_unit,
        column: CHAN_ASSIGNMENT_COLUMN,
        line: 1,
        module: 26,
        line_count: 1,
    };
    assert_eq!(
        encode_var_spec(&wire),
        CHAN_ASSIGNMENT_LAYOUT_DERIVED.to_vec(),
        "chanAssignment 布局推导字节（非抓包）"
    );

    // ② 语义路径：用户字段 → wire，字节必须完全一致。
    let addr =
        NativeNckAddress::from_parameters(&chan_assignment_params(CHAN_ASSIGNMENT_COLUMN, 1))
            .expect("N/1/21/1 地址应合法");
    let rp = resolve_native(addr).expect("N/1/21/1 resolve 应成功");
    assert_eq!(
        hex(&encode_var_spec(&rp.wire)),
        "12 08 82 01 00 15 00 01 1a 01",
        "语义路径与结构化路径必须给出同一 10 字节"
    );

    // 结构侧同步钉住（任一字段漂移都会让上面失败，这里再固定语义值）。
    assert_eq!(rp.wire.syntax_id, 0x82, "current 单位 → 0x82");
    assert_eq!(rp.wire.area_unit, 0x01, "area N(0)<<5 | area_no 1");
    assert_eq!(rp.wire.column, 21);
    assert_eq!(rp.wire.line, 1);
    assert_eq!(rp.wire.module, 26);
    assert_eq!(rp.wire.line_count, 1);

    // `TYPE_UWORD` 是 16 位 → 单点期望响应长度 2 字节；transport 沿用既有 0x09。
    assert_eq!(rp.kind, NckDataKind::U16Le);
    assert_eq!(rp.expected_data_len, 2, "1 × U16_LE = 2 字节");
    assert_eq!(rp.expected_transport_size, 0x09);
    assert_eq!(
        rp.data_type,
        mesa_core_types::DataType::U32,
        "U16 解码后提升为 Core 的 U32"
    );
}

/// 用例 2（反向断言）：`column=21` 编成 `00 15`，**绝不是** `27 1a`（= 10010）。
///
/// 这条用例的存在理由是防一个具体错误：把手册里的 `MD10010` 当成列号。
/// 两者编码只差 `[4..6]` 两个字节，肉眼极易混淆，所以这里既断言“等于什么”，
/// 也断言“不等于什么”，并把 `10010` 的字节形态一起钉住作为对照。
#[test]
fn column_21_is_not_the_md_number_10010_layout_derived() {
    let wire_21 = NckWireAddress {
        syntax_id: 0x82,
        area_unit: NckWireAddress::area_unit(NckArea::Nck.index(), 1).unwrap(),
        column: CHAN_ASSIGNMENT_COLUMN,
        line: 1,
        module: 26,
        line_count: 1,
    };
    let wire_10010 = NckWireAddress {
        column: CHAN_ASSIGNMENT_MD_NUMBER,
        ..wire_21
    };

    let bytes_21 = encode_var_spec(&wire_21);
    let bytes_10010 = encode_var_spec(&wire_10010);

    // ① 正向：column 21 = 0x0015。
    assert_eq!(&bytes_21[4..6], &[0x00, 0x15], "column=21 必须是 00 15");
    // ② 反向：不得出现 MD 号的字节形态 0x271A。
    assert_ne!(
        &bytes_21[4..6],
        &[0x27, 0x1a],
        "column=21 被编成了 MD10010 的字节形态，说明把机床数据号当成了列号"
    );
    assert!(
        !bytes_21.windows(2).any(|w| w == [0x27, 0x1a]),
        "chanAssignment 的请求里不得出现 27 1a（MD10010 的数值形态）"
    );

    // ③ 对照：column=10010 才是 27 1a，且与 column=21 只差 [4..6]。
    assert_eq!(
        &bytes_10010[4..6],
        &[0x27, 0x1a],
        "10010 = 0x271A，作为列号编码时才得到 27 1a"
    );
    assert_eq!(
        u16::from_be_bytes([bytes_10010[4], bytes_10010[5]]),
        CHAN_ASSIGNMENT_MD_NUMBER,
        "27 1a 大端还原就是 10010"
    );
    assert_eq!(&bytes_21[..4], &bytes_10010[..4], "前 4 字节相同");
    assert_eq!(&bytes_21[6..], &bytes_10010[6..], "后 4 字节相同");
    assert_ne!(
        hex(&bytes_21),
        hex(&bytes_10010),
        "两个列号必须编码成不同请求，否则会读到同一个变量"
    );
}

/// 用例 3：area_unit 合成——N 区 = `0x01`，并对照 B/C/A/T/V/H 各区。
///
/// 位运算依据 `codec.rs`：`area << 5 | unit`；area 字母↔索引依据 Wireshark
/// `nck_area_names`（N=0 B=1 C=2 A=3 T=4 V=5 H=6）。此用例同时证明
/// **N 区不会被拼成 C 区**（`0x01` vs `0x41`）——这是 `chanAssignment` 最易错的一步。
#[test]
fn area_unit_per_area_layout_derived() {
    let cases = [
        (NckArea::Nck, 0x01u8, "N"),
        (NckArea::ModeGroup, 0x21, "B"),
        (NckArea::Channel, 0x41, "C"),
        (NckArea::Axis, 0x61, "A"),
        (NckArea::Tool, 0x81, "T"),
        (NckArea::FeedDrive, 0xa1, "V"),
        (NckArea::MainDrive, 0xc1, "H"),
    ];
    for (area, expect, letter) in cases {
        let unit = NckWireAddress::area_unit(area.index(), 1).unwrap();
        assert_eq!(
            unit, expect,
            "{letter} 区 area_no=1 的 area_unit 应为 {expect:#04x}"
        );
        // 该值必须真的出现在编码结果的第 4 个字节（索引 3）。
        let wire = NckWireAddress {
            syntax_id: 0x82,
            area_unit: unit,
            column: CHAN_ASSIGNMENT_COLUMN,
            line: 1,
            module: 26,
            line_count: 1,
        };
        assert_eq!(encode_var_spec(&wire)[3], expect, "{letter} 区第 4 字节");
    }
    // N 区与 C 区只差 area 位，绝不能互相顶替。
    assert_ne!(
        NckWireAddress::area_unit(NckArea::Nck.index(), 1).unwrap(),
        NckWireAddress::area_unit(NckArea::Channel.index(), 1).unwrap(),
        "N 区与 C 区 area_unit 必须不同"
    );
}

/// 用例 4：line 变化只影响 `[6..8]`，`[4..6]`（column）逐字不变。
///
/// 手册说 `Line index: Channel number`，即通道号进 line 槽位；本用例证明改通道号
/// 不会污染 column 字段（`column` 与 `line` 相邻且都是 u16，错位时会静默读错变量）。
#[test]
fn line_changes_only_touch_line_field_layout_derived() {
    let base_area_unit = NckWireAddress::area_unit(NckArea::Nck.index(), 1).unwrap();
    let expected = [
        (1u16, "12 08 82 01 00 15 00 01 1a 01"),
        (2, "12 08 82 01 00 15 00 02 1a 01"),
        (31, "12 08 82 01 00 15 00 1f 1a 01"),
    ];
    for (line, expect) in expected {
        let wire = NckWireAddress {
            syntax_id: 0x82,
            area_unit: base_area_unit,
            column: CHAN_ASSIGNMENT_COLUMN,
            line,
            module: 26,
            line_count: 1,
        };
        let bytes = encode_var_spec(&wire);
        assert_eq!(hex(&bytes), expect, "line={line}");
        // column 字段逐字不变（这是本用例的核心断言）。
        assert_eq!(
            &bytes[4..6],
            &[0x00, 0x15],
            "line={line} 时 column 仍为 00 15"
        );
        // 只有 [6..8] 随 line 变化。
        assert_eq!(
            &bytes[6..8],
            &line.to_be_bytes(),
            "line={line} 必须大端落在 [6..8]"
        );
        assert_eq!(
            &bytes[..4],
            &CHAN_ASSIGNMENT_LAYOUT_DERIVED[..4],
            "前 4 字节不变"
        );
        assert_eq!(
            &bytes[8..],
            &CHAN_ASSIGNMENT_LAYOUT_DERIVED[8..],
            "后 2 字节不变"
        );
    }
}

/// 用例 5：column 边界值的大端编码（0 / 21 / 10000 / 65535）。
///
/// `column` 是 u16 大端：`[4]` 高字节、`[5]` 低字节。10000 是 N 区 MD 号族的
/// 典型值（如 `axis.name`），用来与 `chanAssignment` 的 21 形成对照。
#[test]
fn column_boundaries_big_endian_layout_derived() {
    let area_unit = NckWireAddress::area_unit(NckArea::Nck.index(), 1).unwrap();
    let cases = [
        (0u16, [0x00u8, 0x00]),
        (CHAN_ASSIGNMENT_COLUMN, [0x00, 0x15]),
        (10000, [0x27, 0x10]),
        (u16::MAX, [0xff, 0xff]),
    ];
    for (column, expect_pair) in cases {
        let wire = NckWireAddress {
            syntax_id: 0x82,
            area_unit,
            column,
            line: 1,
            module: 26,
            line_count: 1,
        };
        let bytes = encode_var_spec(&wire);
        assert_eq!(&bytes[4..6], &expect_pair, "column={column} 的大端字节");
        // 大端还原必须回到原值（防止大小端写反）。
        assert_eq!(
            u16::from_be_bytes([bytes[4], bytes[5]]),
            column,
            "column={column} 大端还原"
        );
    }
}

/// 用例 6：area_no 边界（0 / 1 / 31 合法，32 必须报错）。
///
/// 32 走两条既有 fail-closed 路径，两条都必须报错：
/// ① 语义层 `NativeNckAddress::from_parameters` 先按 `MAX_AREA_NO=31` 拦下
///    （`NativeError::OutOfRange`）；
/// ② 编码层 `NckWireAddress::area_unit` 独立再拦一次（`CodecError::BadUnit`），
///    保证即使有人绕过语义层直接构造 wire 也编不出越界 area_unit。
#[test]
fn area_no_boundaries_layout_derived() {
    for (area_no, expect) in [(0u8, 0x00u8), (1, 0x01), (31, 0x1f)] {
        assert_eq!(
            NckWireAddress::area_unit(NckArea::Nck.index(), area_no as u16).unwrap(),
            expect,
            "N 区 area_no={area_no}"
        );
    }

    // ② 编码层错误路径：unit 5 位上限 31。
    assert_eq!(
        NckWireAddress::area_unit(NckArea::Nck.index(), 32).unwrap_err(),
        CodecError::BadUnit { unit: 32 },
        "area_no=32 必须在编码层被拒"
    );

    // ① 语义层错误路径：字段范围检查先行拦截。
    let mut p = chan_assignment_params(CHAN_ASSIGNMENT_COLUMN, 1);
    p.insert("area_no".into(), json!(32));
    let err = NativeNckAddress::from_parameters(&p).expect_err("area_no=32 必须被拒");
    assert!(
        matches!(
            err,
            mesa_driver_sinumerik_nck::NativeError::OutOfRange { .. }
        ),
        "area_no=32 应报 OutOfRange，实际 {err:?}"
    );

    // 合法边界仍能编出正确字节（area_no=31 → 0x1f）。
    let wire = NckWireAddress {
        syntax_id: 0x82,
        area_unit: NckWireAddress::area_unit(NckArea::Nck.index(), 31).unwrap(),
        column: CHAN_ASSIGNMENT_COLUMN,
        line: 1,
        module: 26,
        line_count: 1,
    };
    assert_eq!(
        hex(&encode_var_spec(&wire)),
        "12 08 82 1f 00 15 00 01 1a 01",
        "area_no=31 的 N 区编码"
    );
}

// ---------------------------------------------------------------------------
// 证据边界守卫（防止布局推导值被冒充成真机证据）
// ---------------------------------------------------------------------------

/// 真机证据矩阵（`docs/evidence/` 下的 json；本测试只读，不写）。
const EVIDENCE_JSON: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/evidence/nck-native-address-evidence.json"
);
/// 真机证据目录（本测试只读）。
const EVIDENCE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/evidence");
/// 已观测 block 表源码（真机证据集合；本文件不得往里加推导值）。
const BLOCK_GOLDEN_SRC: &str = include_str!("block_wire_golden.rs");
/// 唯一允许提到 `chanAssignment` 的证据文件：一次性真机只读验证的逐字记录。
///
/// 它记录的是**设备真实返回**（`device-observed`），不是推导值；其结论为
/// “载荷与手册 `TYPE_UWORD` 不符 ⇒ 待核实”，因此不构成“推导值被抬升成证据”。
const DEVICE_OBSERVED_FILE: &str = "nck-live-chan-assignment-probe.txt";

/// 把空白去掉的小写十六进制形态，便于跨排版比对（`12 08 82 …` ≡ `120882…`）。
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 布局推导向量**不得**冒充真机证据（本文件的核心边界断言）。
///
/// 四条独立检查：
/// 1. `block_wire_golden.rs` 的已观测 block 表里不得出现本文件的期望字节；
/// 2. 证据矩阵 `block_observations` 里不得出现 N 区（`area_unit` = 0x01）观测；
/// 3. `docs/evidence/` 下除 `DEVICE_OBSERVED_FILE` 外的任何文件都不得提到
///    `chanAssignment` 或本文件的字节串；
/// 4. `DEVICE_OBSERVED_FILE` 本身必须是"如实、不抬升"的记录（见下方断言）。
///
/// 这是「正向断言已知状态」的守卫写法（同 `block_wire_golden.rs` 的
/// `unobserved_blocks_must_not_get_golden_bytes`）：一旦有人把推导值写进证据，
/// 本测试立刻失败，从而不可能悄悄把证据等级从「推导」抬到「真机」。
#[test]
fn layout_derived_vector_is_absent_from_device_evidence() {
    let target = squash(&hex(&CHAN_ASSIGNMENT_LAYOUT_DERIVED));
    assert_eq!(target, "12088201001500011a01");

    // 1) 已观测 block 表不含本向量（该表只收真机观测）。
    let golden = squash(BLOCK_GOLDEN_SRC);
    assert!(
        !golden.contains(&target),
        "布局推导的 chanAssignment 字节出现在 block_wire_golden.rs 的已观测表里——\
         该表只允许真机观测，不得混入推导值"
    );

    // 2) 证据矩阵里没有任何 N 区观测（现有 5 个 block 全为 C 区 0x41）。
    let raw = std::fs::read_to_string(EVIDENCE_JSON)
        .unwrap_or_else(|e| panic!("读不到证据矩阵 {EVIDENCE_JSON}: {e}"));
    let doc: Value = serde_json::from_str(&raw).expect("证据矩阵必须是合法 JSON");
    let rows = doc["block_observations"]
        .as_array()
        .expect("block_observations 必须是数组");
    for row in rows {
        let block = row["block"].as_str().unwrap_or("<unknown>");
        let area = row["device_observation"]["decoded"]["area"]
            .as_str()
            .unwrap_or("");
        assert_ne!(
            area, "N",
            "block {block} 声称有 N 区真机观测，与现有证据不符"
        );
        let raw_request = row["raw_request_example"].as_str().unwrap_or("");
        assert_ne!(
            squash(raw_request),
            target,
            "block {block} 的观测字节与本文件的推导字节相同，二者必须来自不同来源"
        );
        assert!(
            !squash(raw_request).starts_with("12088201"),
            "block {block} 不得是 N 区（area_unit=0x01）观测"
        );
    }

    // 3) 除真机验证记录外，证据目录里没有任何文件把 chanAssignment 当已观测事实记录。
    let mut checked = 0usize;
    for entry in std::fs::read_dir(EVIDENCE_DIR)
        .unwrap_or_else(|e| panic!("读不到证据目录 {EVIDENCE_DIR}: {e}"))
    {
        let path = entry.expect("证据目录项").path();
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("读不到 {path:?}: {e}"));
        let text = squash(&String::from_utf8_lossy(&bytes));
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // 该文件由第 4 段单独校验（它记录的是设备返回，不是本文件的推导值）。
        if name == DEVICE_OBSERVED_FILE {
            continue;
        }
        assert!(
            !text.contains("chanassignment"),
            "证据文件 {name} 提到 chanAssignment：除 {DEVICE_OBSERVED_FILE} 外没有真机证据，\
             不得把它写成设备观测"
        );
        assert!(
            !text.contains(&target),
            "证据文件 {name} 含本文件的推导字节串：推导值不得进入真机证据集合"
        );
        checked += 1;
    }
    assert!(
        checked >= 5,
        "证据目录应至少检查到 5 个文件，实际 {checked}"
    );

    // 4) 唯一的真机验证记录：允许提到 chanAssignment，但必须"如实、不抬升"。
    //
    // 这些断言的方向与原禁令一致——真机证据可以记录**设备返回了什么**，
    // 但不得把它写成"分配关系已确认"（那需要独立的配置对照证据，本轮没有）。
    let path = std::path::Path::new(EVIDENCE_DIR).join(DEVICE_OBSERVED_FILE);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("真机证据文件 {DEVICE_OBSERVED_FILE} 读不到: {e}"));
    let text = squash(&raw);
    assert!(
        text.contains("device-observed"),
        "{DEVICE_OBSERVED_FILE} 必须标注 device-observed（真机实测），不得与推导值混淆"
    );
    assert!(
        text.contains(&target),
        "{DEVICE_OBSERVED_FILE} 必须逐字记录 chanAssignment 的请求字节 {target}"
    );
    // 设备实际返回 64 字节全 0（见证据文件 §5.2）：必须逐字留存，不能只写"读到了"。
    assert!(
        text.contains(&"00".repeat(64)),
        "{DEVICE_OBSERVED_FILE} 必须逐字记录设备返回的 64 字节全 0 载荷"
    );
    assert!(
        text.contains("undetermined"),
        "{DEVICE_OBSERVED_FILE} 必须保留 UNDETERMINED（结果待核实）的判定"
    );
    // 反向断言：不得出现任何具体的方式组分配结果（`ASSIGNED(1)` 之类）。
    // 本轮没有读出过任何一个 >0 的值，写出来就是编造。
    for d in 0..=9u8 {
        assert!(
            !text.contains(&format!("assigned({d}")),
            "{DEVICE_OBSERVED_FILE} 出现了具体分配结果 assigned({d}…)：本轮没有任何 >0 的读数，属编造"
        );
    }
}
