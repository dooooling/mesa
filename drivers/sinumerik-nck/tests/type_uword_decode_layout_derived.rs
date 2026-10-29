//! `TYPE_UWORD` 解码可行性：**布局推导向量**，非设备证据。
//!
//! 证据标记：`layout-derived`（布局推导）。本文件所有用例都带这个标记，
//! 便于全局检索时与 `docs/evidence/` 下的真机证据区分。
//!
//! ## 本文件的证据等级（必读，不得抬升）
//!
//! 本文件为**布局推导向量**（`layout-derived`），非设备证据：它只证明 Mesa 的解码器**能处理**
//! 「16 位无符号、小端」这种数据格式，**不证明**真实 828D 在
//! `chanAssignment`（N 区 `column=21`）上真的按这个格式返回。
//! 因此本文件不写入 `docs/evidence/`，也不进入 `block_wire_golden.rs` 的观测表。
//!
//! ## `TYPE_UWORD` 对应哪个 `NckDataKind`（读代码确认，不是猜）
//!
//! 手册（`828D_ncvar_lists_man_0325_en-US.pdf` p1065）只给出类型名
//! `TYPE_UWORD`，**不给出字节序**。仓内 16 位无符号有两个候选：
//!
//! | kind | 定义处 | 解码处 | 词表名 |
//! |---|---|---|---|
//! | `NckDataKind::U16Le` | `src/value.rs:25` | `src/value.rs:251` | `U16_LE` |
//! | `NckDataKind::U16Be` | `src/value.rs:27` | `src/value.rs:253` | `U16_BE` |
//!
//! 仓内既有映射是**按区族**区分的（这是既有事实，不是本轮新决策）：
//! - N/B/C 区 NC 变量（`TYPE_UWORD` 条目：`mode_group.op_mode`、
//!   `channel.program.status`、`channel.tool.number_active`）在 `definitions/828d/r1.json`
//!   里都声明 `decode = U16_LE`；
//! - V 区驱动参数（SINAMICS 参数手册族，如 `drive.parameter.r0103`）声明 `U16_BE`。
//!
//! `chanAssignment` 属 N 区 NC 变量族，按同一约定应为 `U16_LE`。
//!
//! NOTE: 「N 区 NC 变量 = 小端」这一条在仓内**没有真机字节级证据**支撑（现有真机
//! 观测只有 `U16_LE` 的 C 区 `progStatus` 返回 `U32(3)`，见
//! `docs/evidence/nck-live-samples-probe.txt`）。真机确认前，本条只作为
//! 编码/解码可行性的假设，不得写进定义目录或探针表。
//!
//! ## 边界
//!
//! 不连机床：全部是纯计算（`decode_value` 对构造好的字节切片解码）。

use mesa_core_types::Value;
use mesa_driver_sinumerik_nck::{NckDataKind, ValueError, decode_value};

/// 冻结的定义目录（只读引用，本文件不修改它）。
const R1_JSON: &str = include_str!("../definitions/828d/r1.json");

/// `TYPE_UWORD` 在仓内 NC 变量族对应的解码种类。
const TYPE_UWORD_KIND: NckDataKind = NckDataKind::U16Le;

/// 确认 `TYPE_UWORD` → `U16_LE` 的映射来自仓内既有事实，而不是本轮的猜测。
///
/// 断言三件事：
/// 1. `U16_LE` 是 16 位（2 字节）、无符号、投影到 Core 的 `U32`；
/// 2. `U16_BE` 同样 16 位无符号（宽度相同，仅字节序不同）——两者都可能承载
///    `TYPE_UWORD`，选哪个是**字节序声明**问题，必须显式写死；
/// 3. `r1.json` 里三条手册标为 `TYPE_UWORD` 的定义确实都写 `U16_LE`，
///    即 N/B/C 区 NC 变量族的既有约定。
#[test]
fn type_uword_maps_to_u16_le_by_existing_catalog_convention_layout_derived() {
    // ① 宽度/符号/投影。
    assert_eq!(TYPE_UWORD_KIND.byte_len(), 2, "UWORD = 16 位 = 2 字节");
    assert_eq!(TYPE_UWORD_KIND.decode_name(), "U16_LE");
    assert_eq!(
        TYPE_UWORD_KIND.core_type(),
        mesa_core_types::DataType::U32,
        "U16 解码后提升到 Core 的 U32（value.rs 注释：避免引入 Core 类型分支）"
    );
    assert!(TYPE_UWORD_KIND.is_numeric(), "UWORD 属数值型");

    // ② 大端变体宽度相同，只是字节序不同——所以必须显式声明，不能靠猜。
    assert_eq!(NckDataKind::U16Be.byte_len(), 2);
    assert_eq!(NckDataKind::U16Be.decode_name(), "U16_BE");
    assert_ne!(
        NckDataKind::U16Le.decode_name(),
        NckDataKind::U16Be.decode_name(),
        "两个 16 位无符号变体的规范名必须可区分"
    );

    // ③ 既有定义目录的约定：手册标 TYPE_UWORD 的 NC 变量条目均为 U16_LE。
    let catalog: serde_json::Value =
        serde_json::from_str(R1_JSON).expect("r1.json 必须是合法 JSON");
    let defs = catalog["definitions"].as_array().expect("definitions 数组");
    for variable_id in [
        "mode_group.op_mode",         // 手册 p89–90：TYPE_UWORD / Column index 3
        "channel.program.status",     // 手册 p306：TYPE_UWORD / Column index 13
        "channel.tool.number_active", // 手册 p277：TYPE_UWORD / Column index 23
    ] {
        let def = defs
            .iter()
            .find(|d| d["variable_id"].as_str() == Some(variable_id))
            .unwrap_or_else(|| panic!("r1.json 缺少 `{variable_id}`"));
        assert_eq!(
            def["decode"].as_str(),
            Some("U16_LE"),
            "`{variable_id}` 是手册 TYPE_UWORD 条目，仓内约定为 U16_LE"
        );
    }
}

/// 边界值：`0`（手册语义 = 通道不存在）、`1`、`31`、`65535`。
///
/// 注意 `0` 的处理方式：解码器**不特判** `0`，它返回 `U32(0)` 而不是 BAD/None。
/// 「0 = channel does not exist」是**使用方的语义**，不是解码层的事实；
/// 若未来真的接 `chanAssignment`，这个区分必须在采集侧显式处理。
#[test]
fn type_uword_boundary_values_layout_derived() {
    let cases = [
        (0u16, "0 = 通道不存在（手册语义，解码层不特判）"),
        (1, "通道 1 属于方式组 1"),
        (31, "方式组号上界形态"),
        (u16::MAX, "UWORD 上界"),
    ];
    for (value, why) in cases {
        // 小端输入：低字节在前。
        let raw = value.to_le_bytes();
        assert_eq!(
            decode_value(&raw, TYPE_UWORD_KIND).unwrap(),
            Value::U32(u32::from(value)),
            "UWORD {value}（{why}）"
        );
    }

    // `0` 必须是「合法的 0 值」，不能被解码层变成错误或空值。
    assert_eq!(
        decode_value(&[0x00, 0x00], TYPE_UWORD_KIND).unwrap(),
        Value::U32(0),
        "手册的「0 = 通道不存在」是返回值语义，解码层只负责给出 0"
    );
    // 65535 是 UWORD 上界，不得溢出或被当成 -1。
    assert_eq!(
        decode_value(&[0xff, 0xff], TYPE_UWORD_KIND).unwrap(),
        Value::U32(65_535)
    );
}

/// 字节序：同一对字节按 LE / BE 解码得到不同数值，且 LE 才是仓内约定。
///
/// 这条用例的意义：`TYPE_UWORD` 的宽度由手册确定，字节序**没有**手册依据，
/// 只能靠定义目录显式声明。若字节序写错，`21` 会静默变成 `5376`——数值“看起来
/// 合理”，不会报错，属于最难发现的一类缺陷。
#[test]
fn type_uword_byte_order_is_explicit_layout_derived() {
    // 21 = 0x0015：小端字节序为 15 00。
    assert_eq!(
        decode_value(&[0x15, 0x00], NckDataKind::U16Le).unwrap(),
        Value::U32(21),
        "小端 15 00 → 21"
    );
    // 同样的 15 00 按大端解会得到 0x1500 = 5376（不是 21）。
    assert_eq!(
        decode_value(&[0x15, 0x00], NckDataKind::U16Be).unwrap(),
        Value::U32(5_376),
        "大端 15 00 → 5376"
    );
    // 反过来：00 15 按大端是 21，按小端是 5376。
    assert_eq!(
        decode_value(&[0x00, 0x15], NckDataKind::U16Be).unwrap(),
        Value::U32(21)
    );
    assert_eq!(
        decode_value(&[0x00, 0x15], NckDataKind::U16Le).unwrap(),
        Value::U32(5_376)
    );
    // 对称值（两字节相同）在两种字节序下相等——这也是“写错字节序不易发现”的原因。
    for v in [0x0000u16, 0xffff] {
        let raw = v.to_le_bytes();
        assert_eq!(
            decode_value(&raw, NckDataKind::U16Le).unwrap(),
            decode_value(&raw, NckDataKind::U16Be).unwrap(),
            "对称值 {v:#06x} 两种字节序结果相同"
        );
    }
}

/// 长度检查：1 字节报错，2 字节正常，多余字节被忽略（只看前 2 字节）。
///
/// `decode_value` 只做**下界**检查（`raw.len() < byte_len` 即 `ShortData`），
/// 上界由 `resolve_native` 的 `expected_data_len`（count × byte_len）在采集层把关。
/// 这里把两个行为都钉住，避免有人误以为解码层会拒绝“长了一点”的响应。
#[test]
fn type_uword_length_checks_layout_derived() {
    // 1 字节 → ShortData（need=2, got=1）。
    let err = decode_value(&[0x15], TYPE_UWORD_KIND).expect_err("1 字节必须报错");
    assert_eq!(
        err,
        ValueError::ShortData {
            kind: TYPE_UWORD_KIND,
            need: 2,
            got: 1,
        },
        "1 字节应是 ShortData，而不是被静默补齐"
    );
    // 0 字节 → 同样 ShortData。
    assert!(matches!(
        decode_value(&[], TYPE_UWORD_KIND),
        Err(ValueError::ShortData {
            need: 2,
            got: 0,
            ..
        })
    ));

    // 2 字节 → 正常。
    assert_eq!(
        decode_value(&[0x15, 0x00], TYPE_UWORD_KIND).unwrap(),
        Value::U32(21)
    );

    // 3 字节 → 仍取前 2 字节（解码层不做上界拒绝）。
    assert_eq!(
        decode_value(&[0x15, 0x00, 0xff], TYPE_UWORD_KIND).unwrap(),
        Value::U32(21),
        "解码层只做下界检查，多余字节忽略；上界由 expected_data_len 在采集层把关"
    );
}
