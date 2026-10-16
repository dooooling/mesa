//! 通用点位信封与 `point_key` 通用格式契约（NCK 语义化 Point Definition 重构 PR-A）。
//!
//! 依据：`docs/design/nck-semantic-point-refactor.md` §2.4（Core 侧点位类型）、
//! §4.1.1 E–K（通用键语法与类型标签编码）、§2.8（校验责任表）。
//!
//! # 职责边界（修订 1 / 修订 4，逐条写进测试）
//!
//! - **信封层严格**：`PointSelection` 使用 `#[serde(deny_unknown_fields)]`，字段恰为
//!   `kind` / `point_key` / `parameters` 三个。未知字段**即拒**（`UNKNOWN_FIELD`），
//!   终止既有 `ResourceSelection` 的静默丢弃缺陷。
//! - **参数层不透明**：`parameters` 是 JSON 对象，Core **不解释**其中任何键——
//!   不认识 `variable_id` / `definition_revision` / `target` / `module` / `entry` /
//!   `identity_mode` 等等，也**不得**由 `parameters` 推导 `point_key`。
//! - **无 Point Kind 白名单**：`kind` **只要求非空字符串**，不枚举 `nck_variable` /
//!   `nck_native`。取值语义校验归 NCK Driver（`UNSUPPORTED_POINT_KIND`）。
//! - **无身份推导**：本模块**不提供** `canonical_identity` 之类的 API，不读
//!   `identity_template`，不按 `variable_id` 拼装，不为 Siemens 键排序。
//!   因此「**格式合法但身份错误**」的 Key 在 Core 侧**必须放行**，由 Driver 在
//!   Configure 时以 `POINT_KEY_MISMATCH` 拒绝（PR-B，§2.4.2 D11）。
//!
//! # 本模块提供的通用能力
//!
//! 1. [`validate_point_binding`]：信封层校验（结构 / 未知字段 / 非空 / 参数层类型）；
//! 2. [`validate_point_key`]：`point_key` **通用格式**校验（§4.1.1 E/F/G/H）；
//! 3. [`encode_typed_value`]：类型标签编码器——**只用于判定「规范形式」**，
//!    不得用于身份推导（修订 4）；
//! 4. [`ensure_endpoint_point_keys_unique`]：Endpoint 内跨 Task 的唯一性。
//!
//! 本模块**不含任何 Siemens 专有标识**（`TargetRef` / `DriveObjectRef` / `DriveEntry` /
//! `nck_variable` / `nck_native` / Module 26/130 一律不出现）；唯一允许出现的 NCK 标识是
//! **Task Binding Kind** 常量 [`NCK_POINTS_BINDING_KIND`]（§2.0 术语表：它属于任务层路由，
//! 与点位层 Point Kind 是两个不同层级）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;

use crate::schema::ValidationIssue;

// ---------------------------------------------------------------------------
// NFC 归一（§4.1.1 F）——唯一实现，只允许 NFC
// ---------------------------------------------------------------------------

/// `point_key` 字符串载荷与键本身的 **NFC** 规范等价归一（§4.1.1 F）。
///
/// # 只允许 NFC
///
/// 归一形式由 [`unicode_normalization::UnicodeNormalization::nfc`] 提供，
/// **绝不允许**以 NFKC / NFKD 替代：兼容归一（compatibility normalization）语义不同，
/// 会把**语义上不同的文本**折叠成同一 Key，制造身份碰撞——例如 `ﬁ`(U+FB01) 折叠为
/// `fi`、全角 `Ａ`(U+FF21) 折叠为 `A`、`①`(U+2460) 折叠为 `1`。这些改写属于「文本等价」
/// 之外的业务语义，Core 无权替业务决定。反证用例见 `nfc_identity_coverage_by_script`。
///
/// 同理不做大小写、空白、分隔符层面的任何等价改写：Core 只做**规范等价**。
///
/// # 无失败分支
///
/// 归一恒有定义（任何合法 `&str` 都能归一出结果），因此本函数不返回 `Result`。
/// 设计文档所说「规范化失败即拒绝」在当前实现下等价于「输入不是合法 UTF-8」，
/// 而 `&str` 已由类型系统保证后者。
///
/// # Unicode 数据版本边界（如实表述）
///
/// 规范编码结果取决于依赖内嵌的 Unicode 数据版本（`unicode-normalization 0.1.25`
/// = Unicode 17.0.0）。冻结的 golden 向量只能证明**它所覆盖的那些字符串**在该数据
/// 版本下的 NFC/NFD 等价关系与编码结果，**不能**证明将来任何 Unicode 数据版本升级
/// 都不会改变**其它**字符串的身份。因此：
/// - 依赖版本被 `=` 精确锁定（见 workspace `Cargo.toml`）并由 `Cargo.lock` 固定校验和；
/// - golden 各分类用例**同时冻结编码后的 Key 字面量**（而非只断言「两种写法相等」），
///   任何编码漂移都会让测试**明确失败**而不是静默通过；
/// - `nfc_unicode_data_version_is_pinned` 直接把数据版本纳入断言，并把 fixture 的
///   `unicode_version` 字段变成**可执行**的一致性检查。
fn nfc(input: &str) -> String {
    input.nfc().collect()
}

/// **Task Binding Kind**：NCK 语义点位任务信封（§2.1）。
///
/// 注意术语分层（§2.0）：这是**任务层**的 kind（`DriverBinding.kind`），
/// 由 Core 用于路由；点位层的 **Point Kind** 是 `PointSelection.kind`，
/// Core 不建立其白名单。
pub const NCK_POINTS_BINDING_KIND: &str = "nck.points.v1";

/// `point_key` **规范编码后**的 UTF-8 字节上限（§4.1.1 长度上限）。
///
/// 依据：42 条定义的默认 Key 中最长者 71 字节，256 字节留 ≥3 倍余量；
/// 且 `point_key` 是 SQLite 唯一索引列，超长键恶化 B-tree 扇出。
/// 超限一律拒绝，**不得截断**（截断会制造碰撞）。
pub const POINT_KEY_MAX_BYTES: usize = 256;

/// 对象 / 数组的嵌套深度上限（**根值深度为 1**），超限即拒（§4.1.1 H-2）。
pub const POINT_KEY_MAX_DEPTH: usize = 4;

/// 可精确表示且不跨运行时漂移的整数上界 `2^53 - 1`（§4.1.1 G-3/G-4）。
const MAX_EXACT_INT: i128 = (1i128 << 53) - 1;

/// 非整数的最小绝对值（§4.1.1 G-5）；更小者一律拒绝。
const MIN_NONZERO_ABS: f64 = 1e-9;

/// 点位信封（`DriverBinding.config.points` 的元素，§2.4）。
///
/// Core **只**认识这三个字段；`parameters` 对 Core 完全不透明。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointSelection {
    /// **Point Kind**：只要求非空字符串；Core **不建白名单**（§2.1-2）。
    pub kind: String,
    /// 点位身份：通用格式校验 + Endpoint 内唯一；Core **不推导身份**（§2.4）。
    pub point_key: String,
    /// 参数层：**不透明** JSON 对象，Core 不解释其中任何键（§2.4）。
    pub parameters: Map<String, Value>,
}

/// 点位信封顶层形态（§2.4）：`{"points": [PointSelection, ...]}`。
///
/// 一个 `nck.points.v1` 任务可同时含 `nck_variable` 与 `nck_native` 点位——
/// Core 不因 Point Kind 拒绝、也不要求拆成两个任务（§2.1 评审裁决）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointBinding {
    pub points: Vec<PointSelection>,
}

impl PointBinding {
    /// 反序列化（信封层严格）。调用方通常先用 [`validate_point_binding`] 取得
    /// 精确错误码，再用本函数取得强类型值。
    pub fn from_json(v: &Value) -> Result<Self, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// 错误类型
// ---------------------------------------------------------------------------

/// `point_key` 通用格式校验失败原因（§4.1.1 E–H）。
///
/// 这些变体**只描述通用语法 / 编码**，不含任何身份正确性判定（修订 4）：
/// 「格式合法但语义身份错误」不会在此产生错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointKeyError {
    /// `point_key` 为空。
    Empty,
    /// 规范编码后超过 [`POINT_KEY_MAX_BYTES`]。
    TooLong { bytes: usize },
    /// 未做 NFC 归一（键本身或某个字符串值段）。
    NotNfc,
    /// 键语法错误（段结构、保留前缀、键名、`~<n>` 后缀等）。
    Syntax(String),
    /// 值段不是**规范**类型标签编码（如缺类型标签、前导零、键序未升序、小写十六进制）。
    NotCanonical(String),
    /// 对象 / 数组嵌套超过 [`POINT_KEY_MAX_DEPTH`]。
    DepthExceeded,
    /// 数字为非有限值（NaN / ±Inf）。
    NonFiniteNumber,
    /// 数字超出可精确表示范围，或非整数绝对值小于 `1e-9`。
    NumberOutOfRange,
}

impl std::fmt::Display for PointKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PointKeyError::Empty => write!(f, "point_key 不能为空"),
            PointKeyError::TooLong { bytes } => write!(
                f,
                "point_key 规范编码后 {bytes} 字节，超过上限 {POINT_KEY_MAX_BYTES} 字节（不得截断）"
            ),
            PointKeyError::NotNfc => {
                write!(f, "point_key 未做 NFC 归一（NFC/NFD 必须得到同一 Key）")
            }
            PointKeyError::Syntax(m) => write!(f, "point_key 键语法非法：{m}"),
            PointKeyError::NotCanonical(m) => {
                write!(f, "point_key 值段不是规范类型标签编码：{m}")
            }
            PointKeyError::DepthExceeded => write!(
                f,
                "point_key 对象/数组嵌套深度超过上限 {POINT_KEY_MAX_DEPTH}（根值深度为 1）"
            ),
            PointKeyError::NonFiniteNumber => write!(f, "point_key 值段含非有限数字（NaN/Inf）"),
            PointKeyError::NumberOutOfRange => write!(
                f,
                "point_key 值段数字越界（整数须在 ±(2^53-1) 内；非整数绝对值须 ≥ 1e-9）"
            ),
        }
    }
}

impl std::error::Error for PointKeyError {}

// ---------------------------------------------------------------------------
// 类型标签编码器（§4.1.1 F/G/H）
// ---------------------------------------------------------------------------

/// 类型标签编码（§4.1.1 F/G/H）：把 JSON 值编码为「类型标签 + 规范载荷」。
///
/// **用途限定**：只用于判定「规范形式」与生成预览；**不得**用于身份推导
/// （修订 4：`canonical_identity` 归 NCK Driver，Core 不知道哪些键进入身份）。
///
/// 规则要点：
/// - 标签表恰 7 个：`s`(string) / `i`(integer) / `d`(非整数 number) / `b`(boolean) /
///   `z`(null) / `o`(object) / `a`(array)；
/// - 字符串：NFC 后按 **UTF-8 字节**百分号编码，仅 `A-Za-z0-9-._` 原样输出，
///   其余一律 `%` + **两位大写**十六进制（含 `~` → `%7E`）；
/// - 数字：`1.0` ≡ `1`、`1e2` ≡ `100`、`-0` → `i:0`；越界即拒（不静默改写）；
/// - 对象：键名按**字节序升序**、显式括号、递归；数组：**保序**；
/// - 深度上限 [`POINT_KEY_MAX_DEPTH`]。
pub fn encode_typed_value(value: &Value) -> Result<String, PointKeyError> {
    encode_at_depth(value, 1)
}

fn encode_at_depth(value: &Value, depth: usize) -> Result<String, PointKeyError> {
    if depth > POINT_KEY_MAX_DEPTH {
        return Err(PointKeyError::DepthExceeded);
    }
    Ok(match value {
        Value::Null => "z:".to_string(),
        Value::Bool(b) => format!("b:{b}"),
        Value::String(s) => format!("s:{}", percent_encode(&nfc(s))),
        Value::Number(n) => encode_number(n)?,
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(encode_at_depth(item, depth + 1)?);
            }
            format!("a:[{}]", parts.join(","))
        }
        Value::Object(map) => {
            // 对象键序**无关**：按字节序升序输出（反例 R6/E8）。
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let mut parts = Vec::with_capacity(keys.len());
            for k in keys {
                if !is_key_name(k) {
                    return Err(PointKeyError::Syntax(format!(
                        "对象键名 `{k}` 不匹配 ^[a-z][a-z0-9_]{{0,63}}$（绝不静默编码）"
                    )));
                }
                parts.push(format!("{k}={}", encode_at_depth(&map[k], depth + 1)?));
            }
            format!("o:{{{}}}", parts.join(","))
        }
    })
}

/// 数字规范化（§4.1.1 G，逐条确定，**禁止**依赖运行时默认格式化）。
fn encode_number(n: &serde_json::Number) -> Result<String, PointKeyError> {
    // JSON 整数表示（无小数点/指数）优先按整数处理，避免走 f64 丢精度。
    if let Some(i) = n.as_i64() {
        if (i as i128).abs() > MAX_EXACT_INT {
            return Err(PointKeyError::NumberOutOfRange);
        }
        return Ok(format!("i:{i}"));
    }
    if let Some(u) = n.as_u64() {
        if u as i128 > MAX_EXACT_INT {
            return Err(PointKeyError::NumberOutOfRange);
        }
        return Ok(format!("i:{u}"));
    }
    let f = n.as_f64().ok_or(PointKeyError::NumberOutOfRange)?;
    if !f.is_finite() {
        return Err(PointKeyError::NonFiniteNumber);
    }
    // `-0.0` 与 `0` 必须同一身份（G-2）。
    if f == 0.0 {
        return Ok("i:0".to_string());
    }
    if f == f.trunc() {
        // `1.0` → `i:1`、`1e2` → `i:100`（G-3）；越界整数拒绝而非改写（G-4）。
        if f.abs() > MAX_EXACT_INT as f64 {
            return Err(PointKeyError::NumberOutOfRange);
        }
        return Ok(format!("i:{}", f as i64));
    }
    if f.abs() < MIN_NONZERO_ABS {
        return Err(PointKeyError::NumberOutOfRange);
    }
    // 非整数 ⇒ `d:` + 定点最短往返十进制（禁指数、禁前导零、禁尾随零）。
    // Rust 的 `Display for f64` 恒为定点且为最短往返表示；显式拒绝指数形态兜底。
    let text = format!("{f}");
    if text.contains(['e', 'E']) {
        return Err(PointKeyError::Syntax(format!("d: 不支持指数形式: {text}")));
    }
    Ok(format!("d:{text}"))
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// 字符串载荷编码：对 NFC 后的 **UTF-8 字节**逐个处理。
///
/// 必须对原始字节**一次性**完成，**不允许**「先转义非 ASCII、再补 `%`」的两遍做法
/// （否则与字面 `%XX` 文本碰撞，反例 E2）。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => out.push(*b as char),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0F) as usize] as char);
            }
        }
    }
    out
}

/// 严格百分号解码：`%` 后必须恰为两位**大写**十六进制；其余字节必须在原始字符集内。
fn percent_decode(payload: &str) -> Result<Vec<u8>, PointKeyError> {
    let bytes = payload.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            let hi = *bytes
                .get(i + 1)
                .ok_or_else(|| PointKeyError::NotCanonical("`%` 后缺少两位十六进制".into()))?;
            let lo = *bytes
                .get(i + 2)
                .ok_or_else(|| PointKeyError::NotCanonical("`%` 后缺少两位十六进制".into()))?;
            let hi_v = hex_val(hi).ok_or_else(|| {
                PointKeyError::NotCanonical(format!("`%{}` 不是大写十六进制", hi as char))
            })?;
            let lo_v = hex_val(lo).ok_or_else(|| {
                PointKeyError::NotCanonical(format!("`%{}` 不是大写十六进制", lo as char))
            })?;
            out.push((hi_v << 4) | lo_v);
            i += 3;
        } else if matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_') {
            out.push(b);
            i += 1;
        } else {
            return Err(PointKeyError::NotCanonical(format!(
                "字符串载荷含非法字符 `{}`",
                b as char
            )));
        }
    }
    Ok(out)
}

/// 大写十六进制半字节；小写与小写混排一律不合法（规范形式要求大写，§4.1.1 F）。
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 键名规则（§4.1.1 末段）：`^[a-z][a-z0-9_]{0,63}$`，**不编码**。
fn is_key_name(k: &str) -> bool {
    let b = k.as_bytes();
    if b.is_empty() || b.len() > 64 {
        return false;
    }
    if !b[0].is_ascii_lowercase() {
        return false;
    }
    b[1..]
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// `variable_id` 规则（§2.2）：`^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$`。
///
/// 另加**显式保留前缀**规则（§4.1.1 E）：`variable_id` 不得等于 `native`，
/// 也不得以 `native.` 开头（后者由语法层强制按 `native_key` 解析）。
fn is_variable_id(s: &str) -> bool {
    let mut segs = s.split('.');
    let Some(head) = segs.next() else {
        return false;
    };
    if head.is_empty() || !head.as_bytes()[0].is_ascii_lowercase() {
        return false;
    }
    if !head
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    {
        return false;
    }
    let mut count = 0usize;
    for seg in segs {
        if seg.is_empty() {
            return false;
        }
        if !seg
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return false;
        }
        count += 1;
    }
    // 至少一个点分段（`variable_id` 必含 `.`，故裸 `native` 天然非法）。
    count >= 1
}

// ---------------------------------------------------------------------------
// point_key 通用格式校验（§4.1.1 E/F/G/H）
// ---------------------------------------------------------------------------

/// `point_key` **通用格式**校验（§4.1.1 E–H）。
///
/// 校验项（**只有这些**）：非空、NFC、≤256 UTF-8 字节、通用键语法
/// （`base_key` + 可选 `~<n>` 后缀）、每个值段是**规范类型标签编码**、
/// 段内键名按**字节序严格升序且不重复**、保留前缀规则。
///
/// **不判定身份正确性**（修订 4）：一个格式完全合法但与 `parameters` 语义不符的 Key
/// 在此**通过**，由 NCK Driver 在 Configure 时以 `POINT_KEY_MISMATCH` 拒绝。
pub fn validate_point_key(key: &str) -> Result<(), PointKeyError> {
    if key.is_empty() {
        return Err(PointKeyError::Empty);
    }
    if key.len() > POINT_KEY_MAX_BYTES {
        return Err(PointKeyError::TooLong { bytes: key.len() });
    }
    // 规范 Key 的编码结果恒为 ASCII（非 ASCII 一律被百分号编码），
    // 故「整串未 NFC」只会出现在手写输入里，此处显式拦截。
    if nfc(key) != key {
        return Err(PointKeyError::NotNfc);
    }

    let base = strip_user_suffix(key)?;

    if let Some(rest) = base.strip_prefix("native.") {
        // 保留前缀：`native.` 开头者**必须**匹配 native_key 形态（§4.1.1 E）。
        return validate_native_key(rest);
    }
    validate_semantic_key(base)
}

/// 剥离可选的 `~<n>` 后缀（§4.1.1 K / 附加 1）。
///
/// `~` 在值载荷中恒被编码为 `%7E`，且不出现在键名、`variable_id` 与 native 十进制段中，
/// 因此「末尾 `~<n>`」可被**唯一识别与剥离**（反例 R9/E10）。
fn strip_user_suffix(key: &str) -> Result<&str, PointKeyError> {
    let Some(pos) = key.rfind('~') else {
        return Ok(key);
    };
    let base = &key[..pos];
    let digits = &key[pos + 1..];
    if base.contains('~') {
        return Err(PointKeyError::Syntax(
            "`~` 只能作为末尾后缀标记出现一次（值中的 `~` 必须编码为 %7E）".into(),
        ));
    }
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PointKeyError::Syntax("`~` 后缀必须为十进制数".into()));
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(PointKeyError::Syntax("`~` 后缀禁止前导零".into()));
    }
    let n: u64 = digits
        .parse()
        .map_err(|_| PointKeyError::Syntax("`~` 后缀越界".into()))?;
    if n < 2 {
        return Err(PointKeyError::Syntax("`~` 后缀必须 ≥ 2".into()));
    }
    Ok(base)
}

/// `native_key := "native." area_letters "." dec "." dec "." dec "." dec`（§4.1.1 E）。
///
/// Core 只校验「第 2 段是字母、其余是十进制」，**不判定**该字母是否属于
/// NCK 的合法 area 集合（那是 Driver 的职责，§2.8 #17）。
fn validate_native_key(rest: &str) -> Result<(), PointKeyError> {
    let mut segs = rest.split('.');
    let area = segs
        .next()
        .ok_or_else(|| PointKeyError::Syntax("native key 缺少 area 段".into()))?;
    if area.is_empty() || !area.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(PointKeyError::Syntax(
            "native key 的 area 段必须为非空字母段".into(),
        ));
    }
    for idx in 0..4 {
        let seg = segs.next().ok_or_else(|| {
            PointKeyError::Syntax(format!("native key 缺少第 {} 个十进制段", idx + 1))
        })?;
        if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
            return Err(PointKeyError::Syntax(format!(
                "native key 的第 {} 个十进制段非法: `{seg}`",
                idx + 1
            )));
        }
        if seg.len() > 1 && seg.starts_with('0') {
            return Err(PointKeyError::Syntax(format!(
                "native key 的第 {} 个十进制段禁止前导零: `{seg}`",
                idx + 1
            )));
        }
    }
    if segs.next().is_some() {
        return Err(PointKeyError::Syntax("native key 段数过多".into()));
    }
    Ok(())
}

/// `semantic_key := variable_id [ "#" arglist ] [ "@" targetlist ]`（§4.1.1 E）。
fn validate_semantic_key(key: &str) -> Result<(), PointKeyError> {
    let bytes = key.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_') {
        i += 1;
    }
    let variable_id = &key[..i];
    if !is_variable_id(variable_id) {
        return Err(PointKeyError::Syntax(format!(
            "变量 ID `{variable_id}` 不匹配 ^[a-z][a-z0-9_]*(\\.[a-z0-9_]+)+$"
        )));
    }
    if i < bytes.len() && bytes[i] == b'#' {
        i = parse_assign_list(key, i + 1)?;
    }
    if i < bytes.len() && bytes[i] == b'@' {
        i = parse_assign_list(key, i + 1)?;
    }
    if i != bytes.len() {
        // 按**字符**取非法字符：`i` 之后可能是多字节字符，按字节切片会 panic。
        let ch = key[i..].chars().next().unwrap_or('?');
        return Err(PointKeyError::Syntax(format!(
            "变量 ID 之后出现非法字符 `{ch}`"
        )));
    }
    Ok(())
}

/// 解析 `key=value ("," key=value)*`，返回结束位置。
///
/// 键名必须按**字节序严格升序**且不重复（规范形式；模板顺序即规范顺序，
/// 消费方不得重排，§4.1.1 排序规则）。值段自定界（见 [`parse_value`]），
/// 因此深度 0 的 `,` 是唯一的分隔符、`@` 是 target 段的开始。
fn parse_assign_list(s: &str, start: usize) -> Result<usize, PointKeyError> {
    let bytes = s.as_bytes();
    let mut i = start;
    let mut prev: Option<&str> = None;
    loop {
        let kstart = i;
        while i < bytes.len() && matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'_') {
            i += 1;
        }
        let k = &s[kstart..i];
        if !is_key_name(k) {
            return Err(PointKeyError::Syntax(format!(
                "键名 `{k}` 不匹配 ^[a-z][a-z0-9_]{{0,63}}$"
            )));
        }
        if bytes.get(i) != Some(&b'=') {
            return Err(PointKeyError::Syntax(format!("键名 `{k}` 之后缺少 `=`")));
        }
        i += 1;
        let (canonical, next) = parse_value(s, i, 1)?;
        if &s[i..next] != canonical.as_str() {
            // parse_value 同时返回规范编码，此处比对可拦截一切非规范写法。
            return Err(PointKeyError::NotCanonical(format!(
                "`{}` 不是规范编码（应为 `{canonical}`）",
                &s[i..next]
            )));
        }
        i = next;
        if let Some(p) = prev
            && k.as_bytes() <= p.as_bytes()
        {
            return Err(PointKeyError::Syntax(format!(
                "键名必须按字节序严格升序且不重复：`{p}` 之后出现 `{k}`"
            )));
        }
        prev = Some(k);
        if bytes.get(i) == Some(&b',') {
            i += 1;
        } else {
            return Ok(i);
        }
    }
}

/// 解析一个值段并返回其**规范编码**与结束位置（§4.1.1 F/G/H）。
///
/// 解析是严格的：任何非规范写法（缺标签、前导零、尾随零、键序未升序、小写十六进制、
/// 未 NFC、超深嵌套、越界数字）都会直接报错，因此「解析成功」蕴含「已是规范形式」；
/// 调用方再与原始切片逐字比对作为第二道保险。
fn parse_value(s: &str, start: usize, depth: usize) -> Result<(String, usize), PointKeyError> {
    if depth > POINT_KEY_MAX_DEPTH {
        return Err(PointKeyError::DepthExceeded);
    }
    let bytes = s.as_bytes();
    let tag = *bytes
        .get(start)
        .ok_or_else(|| PointKeyError::Syntax("值段缺失".into()))?;
    if bytes.get(start + 1) != Some(&b':') {
        return Err(PointKeyError::Syntax(
            "值段必须以 `类型标签:` 开头（如 `i:1`）".into(),
        ));
    }
    let p = start + 2;
    match tag {
        b's' => {
            let mut i = p;
            while i < bytes.len()
                && matches!(
                    bytes[i],
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'%'
                )
            {
                i += 1;
            }
            let decoded = percent_decode(&s[p..i])?;
            let text = String::from_utf8(decoded)
                .map_err(|_| PointKeyError::NotCanonical("字符串载荷不是合法 UTF-8".into()))?;
            let normalized = nfc(&text);
            if normalized != text {
                return Err(PointKeyError::NotNfc);
            }
            Ok((format!("s:{}", percent_encode(&text)), i))
        }
        b'i' => {
            let mut i = p;
            let neg = bytes.get(i) == Some(&b'-');
            if neg {
                i += 1;
            }
            let dstart = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let digits = &s[dstart..i];
            if digits.is_empty() {
                return Err(PointKeyError::Syntax("`i:` 缺少数字".into()));
            }
            if digits.len() > 1 && digits.starts_with('0') {
                return Err(PointKeyError::Syntax("`i:` 禁止前导零".into()));
            }
            let mag: u128 = digits
                .parse()
                .map_err(|_| PointKeyError::NumberOutOfRange)?;
            if mag > MAX_EXACT_INT as u128 {
                return Err(PointKeyError::NumberOutOfRange);
            }
            // `-0` 归一到 `0`（规范形式不含 `-0`）。
            let text = if neg && mag != 0 {
                format!("-{mag}")
            } else {
                mag.to_string()
            };
            Ok((format!("i:{text}"), i))
        }
        b'd' => {
            let mut i = p;
            if bytes.get(i) == Some(&b'-') {
                i += 1;
            }
            let istart = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let int_part = &s[istart..i];
            if bytes.get(i) != Some(&b'.') {
                return Err(PointKeyError::Syntax(
                    "`d:` 必须为定点小数（禁止指数形式）".into(),
                ));
            }
            i += 1;
            let fstart = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let frac_part = &s[fstart..i];
            if int_part.is_empty() || frac_part.is_empty() {
                return Err(PointKeyError::Syntax(
                    "`d:` 的整数部分与小数部分都不得为空".into(),
                ));
            }
            if int_part.len() > 1 && int_part.starts_with('0') {
                return Err(PointKeyError::Syntax("`d:` 禁止前导零".into()));
            }
            if frac_part.ends_with('0') {
                return Err(PointKeyError::Syntax("`d:` 禁止尾随零".into()));
            }
            let text = &s[p..i];
            let f: f64 = text
                .parse()
                .map_err(|_| PointKeyError::Syntax(format!("`d:{text}` 不是合法数字")))?;
            // 语法已限定为 `-?\d+\.\d+`，故 `f` 必为有限值；用 `<` 表达「过小即拒」。
            if f.abs() < MIN_NONZERO_ABS {
                return Err(PointKeyError::NumberOutOfRange);
            }
            let canonical = format!("d:{f}");
            Ok((canonical, i))
        }
        b'b' => {
            let rest = &s[p..];
            for (lit, out) in [("true", "b:true"), ("false", "b:false")] {
                if rest.starts_with(lit) {
                    return Ok((out.to_string(), p + lit.len()));
                }
            }
            Err(PointKeyError::Syntax("`b:` 载荷只能是 true / false".into()))
        }
        b'z' => Ok(("z:".to_string(), p)),
        b'o' => {
            let mut i = p;
            if bytes.get(i) != Some(&b'{') {
                return Err(PointKeyError::Syntax("`o:` 缺少 `{`".into()));
            }
            i += 1;
            let mut parts: Vec<String> = Vec::new();
            let mut prev: Option<&str> = None;
            if bytes.get(i) == Some(&b'}') {
                // 空对象：编码器对 `{}` 的产出，故必须被接受。
                return Ok(("o:{}".to_string(), i + 1));
            }
            loop {
                let kstart = i;
                while i < bytes.len() && matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'_') {
                    i += 1;
                }
                let k = &s[kstart..i];
                if !is_key_name(k) {
                    return Err(PointKeyError::Syntax(format!(
                        "对象键名 `{k}` 不匹配 ^[a-z][a-z0-9_]{{0,63}}$"
                    )));
                }
                if let Some(pv) = prev
                    && k.as_bytes() <= pv.as_bytes()
                {
                    return Err(PointKeyError::Syntax(format!(
                        "对象键名必须按字节序严格升序：`{pv}` 之后出现 `{k}`"
                    )));
                }
                prev = Some(k);
                if bytes.get(i) != Some(&b'=') {
                    return Err(PointKeyError::Syntax(format!(
                        "对象键名 `{k}` 之后缺少 `=`"
                    )));
                }
                i += 1;
                let (v, next) = parse_value(s, i, depth + 1)?;
                if &s[i..next] != v.as_str() {
                    return Err(PointKeyError::NotCanonical(format!(
                        "`{}` 不是规范编码（应为 `{v}`）",
                        &s[i..next]
                    )));
                }
                i = next;
                parts.push(format!("{k}={v}"));
                match bytes.get(i) {
                    Some(b',') => i += 1,
                    Some(b'}') => {
                        i += 1;
                        break;
                    }
                    _ => return Err(PointKeyError::Syntax("`o:` 期望 `,` 或 `}`".into())),
                }
            }
            Ok((format!("o:{{{}}}", parts.join(",")), i))
        }
        b'a' => {
            let mut i = p;
            if bytes.get(i) != Some(&b'[') {
                return Err(PointKeyError::Syntax("`a:` 缺少 `[`".into()));
            }
            i += 1;
            let mut parts: Vec<String> = Vec::new();
            if bytes.get(i) == Some(&b']') {
                // 空数组：编码器对 `[]` 的产出。
                return Ok(("a:[]".to_string(), i + 1));
            }
            loop {
                let (v, next) = parse_value(s, i, depth + 1)?;
                if &s[i..next] != v.as_str() {
                    return Err(PointKeyError::NotCanonical(format!(
                        "`{}` 不是规范编码（应为 `{v}`）",
                        &s[i..next]
                    )));
                }
                i = next;
                parts.push(v);
                match bytes.get(i) {
                    Some(b',') => i += 1,
                    Some(b']') => {
                        i += 1;
                        break;
                    }
                    _ => return Err(PointKeyError::Syntax("`a:` 期望 `,` 或 `]`".into())),
                }
            }
            Ok((format!("a:[{}]", parts.join(",")), i))
        }
        other => Err(PointKeyError::Syntax(format!(
            "未知类型标签 `{}`（合法标签恰为 s/i/d/b/z/o/a）",
            other as char
        ))),
    }
}

// ---------------------------------------------------------------------------
// 信封层校验（§2.4 Core 校验清单）
// ---------------------------------------------------------------------------

fn issue(path: String, code: &str, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        path,
        code: code.into(),
        message: message.into(),
    }
}

/// 点位信封**信封层**校验（§2.4）。返回 issues（空 = 通过）。
///
/// 只做信封层：结构、未知字段（`deny_unknown_fields` 语义）、`kind` 非空、
/// `point_key` 通用格式、`parameters` 是 JSON 对象。
/// **不**校验 `parameters` 内部、**不**建立 Point Kind 白名单、**不**推导身份。
///
/// `root` 为信封路径前缀，如 `tasks[0].binding.config`。
pub fn validate_point_binding(config: &Value, root: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let Some(obj) = config.as_object() else {
        issues.push(issue(
            root.to_string(),
            "INVALID_STRUCTURE",
            "binding config 必须是 JSON 对象",
        ));
        return issues;
    };
    // 信封层未知字段即拒——这是「不得静默丢弃」的落点（§2.4）。
    for k in obj.keys() {
        if k != "points" {
            issues.push(issue(
                format!("{root}.{k}"),
                "UNKNOWN_FIELD",
                format!("binding config 不允许字段 `{k}`（信封层严格，未知字段不得静默丢弃）"),
            ));
        }
    }
    let Some(points) = obj.get("points") else {
        issues.push(issue(
            root.to_string(),
            "INVALID_STRUCTURE",
            "binding config 缺少 `points`",
        ));
        return issues;
    };
    let Some(arr) = points.as_array() else {
        issues.push(issue(
            format!("{root}.points"),
            "INVALID_STRUCTURE",
            "`points` 必须是数组",
        ));
        return issues;
    };
    for (i, p) in arr.iter().enumerate() {
        let base = format!("{root}.points[{i}]");
        let Some(po) = p.as_object() else {
            issues.push(issue(base, "INVALID_STRUCTURE", "点位必须是 JSON 对象"));
            continue;
        };
        for k in po.keys() {
            if !matches!(k.as_str(), "kind" | "point_key" | "parameters") {
                issues.push(issue(
                    format!("{base}.{k}"),
                    "UNKNOWN_FIELD",
                    format!("点位不允许字段 `{k}`（信封层恰为 kind/point_key/parameters）"),
                ));
            }
        }
        // kind：**只判非空**，不建白名单（§2.1-2 / P9）。
        match po.get("kind") {
            None => issues.push(issue(
                format!("{base}.kind"),
                "INVALID_STRUCTURE",
                "点位缺少 `kind`",
            )),
            Some(Value::String(s)) if s.is_empty() => issues.push(issue(
                format!("{base}.kind"),
                "EMPTY_POINT_KIND",
                "`kind` 不能为空字符串",
            )),
            Some(Value::String(_)) => {}
            Some(_) => issues.push(issue(
                format!("{base}.kind"),
                "INVALID_STRUCTURE",
                "`kind` 必须是字符串",
            )),
        }
        // point_key：非空 + 通用格式（不判定身份正确性）。
        match po.get("point_key") {
            None => issues.push(issue(
                format!("{base}.point_key"),
                "INVALID_STRUCTURE",
                "点位缺少 `point_key`",
            )),
            Some(Value::String(s)) if s.is_empty() => issues.push(issue(
                format!("{base}.point_key"),
                "EMPTY_POINT_KEY",
                "`point_key` 不能为空",
            )),
            Some(Value::String(s)) => {
                if let Err(e) = validate_point_key(s) {
                    issues.push(issue(
                        format!("{base}.point_key"),
                        "INVALID_POINT_KEY",
                        e.to_string(),
                    ));
                }
            }
            Some(_) => issues.push(issue(
                format!("{base}.point_key"),
                "INVALID_STRUCTURE",
                "`point_key` 必须是字符串",
            )),
        }
        // parameters：**只判是 JSON 对象**；内容对 Core 完全不透明（§2.4）。
        match po.get("parameters") {
            None => issues.push(issue(
                format!("{base}.parameters"),
                "INVALID_STRUCTURE",
                "点位缺少 `parameters`",
            )),
            Some(Value::Object(_)) => {}
            Some(_) => issues.push(issue(
                format!("{base}.parameters"),
                "INVALID_PARAMETERS",
                "`parameters` 必须是 JSON 对象（Core 不解释其内容）",
            )),
        }
    }
    issues
}

/// 信封层校验 + 反序列化。校验通过时 [`PointBinding::from_json`] 必然成功。
pub fn parse_point_binding(
    config: &Value,
    root: &str,
) -> (Vec<ValidationIssue>, Option<PointBinding>) {
    let issues = validate_point_binding(config, root);
    if !issues.is_empty() {
        return (issues, None);
    }
    match PointBinding::from_json(config) {
        Ok(b) => (Vec::new(), Some(b)),
        Err(e) => (vec![issue(root.to_string(), "INVALID_STRUCTURE", e)], None),
    }
}

/// Endpoint 内 `point_key` 唯一性（**跨 Task**，§2.8 #10 / R5）。
///
/// `entries` 为本次待检的 `(point_key, path)`；`occupied` 为已被**其它**
/// Task Binding Kind 占用的 `(point_key, path)`——两种 kind 的 Key 同处一个
/// Endpoint 命名空间，必须一起判重。同 kind 内部的重复由各自的集合校验负责
/// （`mesa.resources.v1` 走 `validate_task_set_against`）。
pub fn ensure_endpoint_point_keys_unique(
    entries: &[(&str, &str)],
    occupied: &[(&str, &str)],
) -> Vec<ValidationIssue> {
    use std::collections::HashMap;
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for (k, p) in occupied {
        seen.entry(k).or_insert(p);
    }
    let mut issues = Vec::new();
    for (k, p) in entries {
        if let Some(first) = seen.get(k) {
            issues.push(issue(
                (*p).to_string(),
                "DUPLICATE_POINT_KEY",
                format!("point_key `{k}` 与 {first} 重复（endpoint 内必须唯一）"),
            ));
        } else {
            seen.insert(k, p);
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// E1–E11 golden 向量（与 NCK Driver / Frontend **同源**，§6.6 T5）。
    /// 放在仓库级 `tests/fixtures/` 以便 PR-B/PR-C 复用同一份文件。
    const GOLDEN_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/nck-point-key-golden.json"
    ));

    fn golden() -> Value {
        serde_json::from_str(GOLDEN_JSON).expect("golden fixture 必须是合法 JSON")
    }

    fn error_name(e: &PointKeyError) -> &'static str {
        match e {
            PointKeyError::Empty => "Empty",
            PointKeyError::TooLong { .. } => "TooLong",
            PointKeyError::NotNfc => "NotNfc",
            PointKeyError::Syntax(_) => "Syntax",
            PointKeyError::NotCanonical(_) => "NotCanonical",
            PointKeyError::DepthExceeded => "DepthExceeded",
            PointKeyError::NonFiniteNumber => "NonFiniteNumber",
            PointKeyError::NumberOutOfRange => "NumberOutOfRange",
        }
    }

    fn codes(issues: &[ValidationIssue]) -> Vec<String> {
        issues.iter().map(|i| i.code.clone()).collect()
    }

    // -----------------------------------------------------------------------
    // 类型标签编码器（§4.1.1 F/G/H）
    // -----------------------------------------------------------------------

    #[test]
    fn type_tag_table_is_total() {
        // 标签表恰 7 个：s / i / d / b / z / o / a。
        let cases: [(Value, &str); 7] = [
            (json!("s"), "s:s"),
            (json!(1), "i:1"),
            (json!(1.5), "d:1.5"),
            (json!(true), "b:true"),
            (Value::Null, "z:"),
            (json!({"a": 1}), "o:{a=i:1}"),
            (json!([1]), "a:[i:1]"),
        ];
        let tags: Vec<char> = cases
            .iter()
            .map(|(_, e)| e.chars().next().unwrap())
            .collect();
        assert_eq!(tags, vec!['s', 'i', 'd', 'b', 'z', 'o', 'a']);
        for (v, expected) in cases {
            assert_eq!(encode_typed_value(&v).unwrap(), expected);
        }
        // `1` / `"1"` / `true` / `"true"` 四种编码**互不相同**（反例 E6/R1/R4）。
        let enc: Vec<String> = [json!(1), json!("1"), json!(true), json!("true")]
            .iter()
            .map(|v| encode_typed_value(v).unwrap())
            .collect();
        assert_eq!(enc, vec!["i:1", "s:1", "b:true", "s:true"]);
        for i in 0..enc.len() {
            for j in (i + 1)..enc.len() {
                assert_ne!(enc[i], enc[j], "类型标签必须消除碰撞");
            }
        }
        // 未知标签一律拒绝（合法标签恰为 7 个）。
        assert!(matches!(
            validate_point_key("a.b#x=x:1"),
            Err(PointKeyError::Syntax(_))
        ));
    }

    #[test]
    fn number_normalization() {
        // 整数归一（G-2/G-3）：1.0 ≡ 1、1e2 ≡ 100、-0 → i:0。
        assert_eq!(encode_typed_value(&json!(1.0)).unwrap(), "i:1");
        assert_eq!(
            encode_typed_value(&json!(1.0)).unwrap(),
            encode_typed_value(&json!(1)).unwrap()
        );
        assert_eq!(encode_typed_value(&json!(1e2)).unwrap(), "i:100");
        assert_eq!(
            encode_typed_value(&json!(1e2)).unwrap(),
            encode_typed_value(&json!(100)).unwrap()
        );
        assert_eq!(encode_typed_value(&json!(-0.0)).unwrap(), "i:0");
        assert_eq!(
            encode_typed_value(&json!(-0.0)).unwrap(),
            encode_typed_value(&json!(0)).unwrap()
        );
        // 非整数保持 d:，且与 i:0 不同（0.001 ≠ 0）。
        assert_eq!(encode_typed_value(&json!(0.001)).unwrap(), "d:0.001");
        assert_ne!(
            encode_typed_value(&json!(0.001)).unwrap(),
            encode_typed_value(&json!(0)).unwrap()
        );
        // 越界即拒（G-4/G-5）：2^53 与 1e-12。
        assert_eq!(
            encode_typed_value(&json!(9007199254740992i64)).unwrap_err(),
            PointKeyError::NumberOutOfRange
        );
        assert_eq!(
            encode_typed_value(&json!(-9007199254740992i64)).unwrap_err(),
            PointKeyError::NumberOutOfRange
        );
        assert_eq!(
            encode_typed_value(&json!(1e-12)).unwrap_err(),
            PointKeyError::NumberOutOfRange
        );
        // 上界 2^53-1 恰好允许。
        assert_eq!(
            encode_typed_value(&json!(9007199254740991i64)).unwrap(),
            "i:9007199254740991"
        );
        // 禁止指数形态：`d:` 载荷必须是定点十进制（G-6/G-7）。
        for v in [
            json!(1.5),
            json!(0.001),
            json!(1e-9),
            json!(123456.789),
            json!(-2.5),
        ] {
            let e = encode_typed_value(&v).unwrap();
            assert!(e.starts_with("d:"), "{e}");
            assert!(!e.contains(['e', 'E']), "禁止指数形式: {e}");
        }
        // 非有限值拒绝（serde_json 无法构造 NaN/Inf，故直接验证编码器对 f64 的守卫：
        // `Number::from_f64` 对非有限值返回 None，编码器走到 NonFiniteNumber 分支）。
        assert_eq!(
            encode_typed_value(&Value::Number(serde_json::Number::from_f64(1.0).unwrap())).unwrap(),
            "i:1"
        );
    }

    #[test]
    fn percent_encoding_and_tilde() {
        // 分隔符与保留字符必须逐字节百分号编码。
        assert_eq!(
            encode_typed_value(&json!("a,b=c@d#e/f%g")).unwrap(),
            "s:a%2Cb%3Dc%40d%23e%2Ff%25g"
        );
        // `~` 必须编码为 %7E（修订 5 相对上一版的收紧，反例 R9/E10）。
        assert_eq!(encode_typed_value(&json!("~")).unwrap(), "s:%7E");
        assert_eq!(encode_typed_value(&json!("abc~2")).unwrap(), "s:abc%7E2");
        // 十六进制必须**大写**。
        assert_eq!(encode_typed_value(&json!("轴")).unwrap(), "s:%E8%BD%B4");
        // 字面 `%XX` 与真实字节不得碰撞（反例 E2）。
        assert_ne!(
            encode_typed_value(&json!("轴")).unwrap(),
            encode_typed_value(&json!("%E8%BD%B4")).unwrap()
        );
        // 原样输出的字符集恰为 A-Za-z0-9-._
        assert_eq!(encode_typed_value(&json!("Az09-._")).unwrap(), "s:Az09-._");
        // 小写十六进制不是规范形式。
        assert!(validate_point_key("a.b#x=s:%e8%bd%b4").is_err());
    }

    #[test]
    fn nfc_normalization() {
        let nfc_form = "\u{00E9}";
        let nfd_form = "e\u{0301}";
        // NFC/NFD 必须得到**同一** Key（反例 E3）。
        assert_eq!(
            encode_typed_value(&json!(nfc_form)).unwrap(),
            encode_typed_value(&json!(nfd_form)).unwrap()
        );
        assert_eq!(encode_typed_value(&json!(nfc_form)).unwrap(), "s:%C3%A9");
        // Key 层：NFC 形态合法。
        assert!(validate_point_key("channel.r_parameter#name=s:%C3%A9@channel=i:1").is_ok());
        // Key 层：NFD 形态（e%CC%81）不是规范形式 ⇒ 拒绝。
        assert_eq!(
            validate_point_key("channel.r_parameter#name=s:e%CC%81@channel=i:1"),
            Err(PointKeyError::NotNfc)
        );
        // 原始（未百分号编码）非 ASCII 同样不是规范形式。
        assert!(validate_point_key("channel.r_parameter#name=s:é@channel=i:1").is_err());
    }

    /// 单个 NFC 覆盖类别（任务三：评审追加要求）。
    struct NfcCase {
        /// 类别说明（含脚本 / 现象名）。
        what: &'static str,
        /// 写法一。
        form_a: &'static str,
        /// 写法二：与 `form_a` **规范等价**，必须得到同一 Key。
        form_b: &'static str,
        /// 冻结的**规范载荷**字面量（`s:` + 大写百分号编码）。
        frozen_payload: &'static str,
        /// golden fixture 中对应的合法 Key 用例 id（跨实现同源，防两份字面量各自漂移）。
        golden_ok_id: &'static str,
        /// 非规范载荷（必须被 `NotNfc` 拒绝）与其 fixture 用例 id；`None` = 该类别无此形态。
        noncanonical: Option<(&'static str, &'static str)>,
    }

    /// NFC 覆盖边界：代表性用例 + **完整 Key 字面量冻结**。
    ///
    /// # 这组用例能证明什么、不能证明什么（如实表述）
    ///
    /// **能**证明：下述**所列**字符串在本 crate 依赖的 Unicode 数据版本
    /// （`unicode-normalization 0.1.25` ⇒ `UNICODE_VERSION = (17,0,0)`）下，
    /// ① 两种等价写法得到**同一编码**；② 编码结果等于**冻结的字面量**（值载荷 + 完整 Key）；
    /// ③ 规范字面量通过通用格式校验，非规范写法被 `NotNfc` 拒绝。
    ///
    /// **不能**证明：将来任何 Unicode 数据版本升级都不会改变**其它**字符串的身份。
    /// 规范编码是「(依赖版本, 字符串) → 字节」的函数，数据版本一变，边缘字符串的规范形式
    /// 就可能变。因此本用例**不提供**「未来不漂移」的保证，只提供「**漂移必被发现**」的保证：
    /// 每个类别都冻结了编码后的字面量（而非只断言「两者相等」），任何依赖 / 数据版本变化
    /// 导致的编码漂移都会让本用例**明确失败**，而不是静默通过。
    /// 依赖版本由 workspace `Cargo.toml` 的 `=` 精确锁定、由 `Cargo.lock` 固定校验和，
    /// 数据版本本身另由 `nfc_unicode_data_version_is_pinned` 断言守护。
    ///
    /// 覆盖类别：组合字符（含多记号规范排序）、CJK、Cyrillic、阿拉伯文、天城文、
    /// 单例分解消去、Hangul 算术组合。NFC≠NFKC 的正面反证见 `nfc_is_not_nfkc`。
    #[test]
    fn nfc_identity_coverage_by_script() {
        const CASES: &[NfcCase] = &[
            // ① 组合字符：基字符 + 组合用记号 U+0301 ≡ 预组合 U+00E9（E3 同类扩面）
            NfcCase {
                what: "组合字符（拉丁：基字符+U+0301）",
                form_a: "\u{00E9}",
                form_b: "e\u{0301}",
                frozen_payload: "s:%C3%A9",
                golden_ok_id: "nfc-key-latin",
                noncanonical: Some(("s:e%CC%81", "E3-nfd-value")),
            },
            // ① b 多组合记号：必须按 ccc **升序重排**后再组合
            //      （U+0327 ccc=202 < U+0301 ccc=230，故 a+U+0301+U+0327 → á + U+0327）
            NfcCase {
                what: "组合字符（多记号 + 规范排序）",
                form_a: "\u{00E1}\u{0327}",
                form_b: "a\u{0301}\u{0327}",
                frozen_payload: "s:%C3%A1%CC%A7",
                golden_ok_id: "nfc-key-latin-multi-mark",
                noncanonical: Some(("s:a%CC%81%CC%A7", "nfc-reject-multi-mark-unsorted")),
            },
            // ② 非拉丁：CJK（汉字 U+6C49）。该码点无分解，两种写法同一，故无 NFD 反例。
            NfcCase {
                what: "非拉丁 CJK（汉字）",
                form_a: "\u{6C49}",
                form_b: "\u{6C49}",
                frozen_payload: "s:%E6%B1%89",
                golden_ok_id: "nfc-key-cjk",
                noncanonical: None,
            },
            // ③ 非拉丁：Cyrillic。U+0438 + U+0306(breve) ≡ 预组合 U+0439(й)
            NfcCase {
                what: "非拉丁 Cyrillic（含组合记号）",
                form_a: "\u{0439}",
                form_b: "\u{0438}\u{0306}",
                frozen_payload: "s:%D0%B9",
                golden_ok_id: "nfc-key-cyrillic",
                noncanonical: Some(("s:%D0%B8%CC%86", "nfc-reject-cyrillic-decomposed")),
            },
            // ④ 非拉丁：阿拉伯文。U+0627 + U+0654(hamza above) ≡ 预组合 U+0623(أ)
            NfcCase {
                what: "非拉丁阿拉伯文（含组合记号）",
                form_a: "\u{0623}",
                form_b: "\u{0627}\u{0654}",
                frozen_payload: "s:%D8%A3",
                golden_ok_id: "nfc-key-arabic",
                noncanonical: Some(("s:%D8%A7%D9%94", "nfc-reject-arabic-decomposed")),
            },
            // ⑤ 非拉丁：天城文。U+0958 属**组合排除**字符 ⇒ 只分解不重组，
            //      故其规范形式是「क + nukta」两码点；预组合写法 U+0958 本身非规范。
            NfcCase {
                what: "非拉丁天城文（组合排除：只分解不重组）",
                form_a: "\u{0915}\u{093C}",
                form_b: "\u{0958}",
                frozen_payload: "s:%E0%A4%95%E0%A4%BC",
                golden_ok_id: "nfc-key-devanagari",
                noncanonical: Some(("s:%E0%A5%98", "nfc-reject-devanagari-precomposed")),
            },
            // ⑥ 单例**规范**分解消去：U+212B ANGSTROM SIGN → U+00C5
            NfcCase {
                what: "单例规范分解消去（U+212B → U+00C5）",
                form_a: "\u{00C5}",
                form_b: "\u{212B}",
                frozen_payload: "s:%C3%85",
                golden_ok_id: "nfc-key-singleton",
                noncanonical: Some(("s:%E2%84%AB", "nfc-reject-singleton")),
            },
            // ⑦ Hangul 算术分解/组合：L+V+T → U+AC01
            NfcCase {
                what: "Hangul 算术组合（L+V+T → U+AC01）",
                form_a: "\u{AC01}",
                form_b: "\u{1100}\u{1161}\u{11A8}",
                frozen_payload: "s:%EA%B0%81",
                golden_ok_id: "nfc-key-hangul",
                noncanonical: Some(("s:%E1%84%80%E1%85%A1%E1%86%A8", "nfc-reject-hangul-jamo")),
            },
        ];
        assert!(CASES.len() >= 8, "NFC 覆盖类别不得减少");

        let g = golden();
        // 按 id 取 fixture 用例（key_cases），用于交叉核对字面量。
        let fixture_case = |id: &str| -> Value {
            g["key_cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .unwrap_or_else(|| panic!("golden fixture 缺少 key case `{id}`"))
                .clone()
        };

        for c in CASES {
            // ① 两种规范等价写法必须得到**同一**编码，且等于冻结字面量。
            let a = encode_typed_value(&json!(c.form_a))
                .unwrap_or_else(|e| panic!("{}：写法一编码失败 {e}", c.what));
            let b = encode_typed_value(&json!(c.form_b))
                .unwrap_or_else(|e| panic!("{}：写法二编码失败 {e}", c.what));
            assert_eq!(a, c.frozen_payload, "{}：写法一编码漂移", c.what);
            assert_eq!(b, c.frozen_payload, "{}：写法二必须归一到同一编码", c.what);
            assert_eq!(a, b, "{}：NFC 等价写法必须同一 Key", c.what);

            // ② **完整 Key 字面量**冻结：由冻结载荷构造的 Key 必须通过通用格式校验，
            //    且与 cross-implementation fixture 中的字面量**逐字一致**。
            let key = format!("channel.r_parameter#name={}@channel=i:1", c.frozen_payload);
            assert_eq!(
                validate_point_key(&key),
                Ok(()),
                "{}：冻结的规范 Key 字面量必须通过（{key}）",
                c.what
            );
            let fc = fixture_case(c.golden_ok_id);
            assert_eq!(fc["valid"], json!(true), "{}：fixture 用例应合法", c.what);
            assert_eq!(
                fc["key"].as_str().unwrap(),
                key,
                "{}：fixture 字面量与用例字面量必须逐字一致（防两份字面量各自漂移）",
                c.what
            );

            // ③ 非规范写法必须被明确拒绝为 `NotNfc`——这条证明「NFC 真的作用了」，
            //    而不是「归一被跳过、恰好字面量相同」。
            if let Some((nc_payload, nc_id)) = c.noncanonical {
                assert_ne!(
                    nc_payload, c.frozen_payload,
                    "{}：非规范载荷不得与规范载荷相同（否则该反例无效）",
                    c.what
                );
                let nc_key = format!("channel.r_parameter#name={nc_payload}@channel=i:1");
                assert_eq!(
                    validate_point_key(&nc_key).unwrap_err(),
                    PointKeyError::NotNfc,
                    "{}：非规范写法必须被拒为 NotNfc（{nc_key}）",
                    c.what
                );
                let f = fixture_case(nc_id);
                assert_eq!(f["valid"], json!(false), "{}：fixture 反例应非法", c.what);
                assert_eq!(f["error"].as_str().unwrap(), "NotNfc", "{}", c.what);
                assert_eq!(f["key"].as_str().unwrap(), nc_key, "{}", c.what);
            }
        }
    }

    /// **NFC ≠ NFKC 反证**（评审要求：绝不用 NFKC 替代）。
    ///
    /// 兼容归一（NFKC/NFKD）会把语义不同的文本折叠为同一文本，制造身份碰撞。
    /// 本用例不调用 NFKC，而是直接用「若误用 NFKC 就会碰撞」的**成对字面量**反证：
    /// 实现一旦改用 NFKC，两个编码就会相等，本断言立刻失败。
    #[test]
    fn nfc_is_not_nfkc() {
        // `ﬁ`(U+FB01 LATIN SMALL LIGATURE FI) 只有**兼容**分解为 `fi`；
        // 全角 `Ａ`(U+FF21) 只有**兼容**分解为 `A`。规范形式必须保持原码点。
        const PAIRS: &[(&str, &str, &str, &str, &str, &str)] = &[
            // (说明, 兼容形式, 其 NFKC 折叠结果, 兼容形式的规范载荷, 折叠结果的载荷, fixture key 用例 id)
            (
                "fi 连字 U+FB01",
                "\u{FB01}",
                "fi",
                "s:%EF%AC%81",
                "s:fi",
                "nfc-key-ligature-kept",
            ),
            (
                "全角 Ａ U+FF21",
                "\u{FF21}",
                "A",
                "s:%EF%BC%A1",
                "s:A",
                "nfc-key-fullwidth-kept",
            ),
        ];
        let g = golden();
        for (what, compat_form, folded_form, compat_payload, folded_payload, ok_id) in PAIRS {
            assert_eq!(
                encode_typed_value(&json!(compat_form)).unwrap(),
                *compat_payload,
                "{what}：NFC 必须保持原码点（若改用 NFKC 会得到 {folded_payload}）"
            );
            assert_eq!(
                encode_typed_value(&json!(folded_form)).unwrap(),
                *folded_payload,
                "{what}：折叠形态自身编码"
            );
            // 关键断言：两者**必须不同**。误用 NFKC ⇒ 相等 ⇒ 本行失败。
            assert_ne!(
                encode_typed_value(&json!(compat_form)).unwrap(),
                encode_typed_value(&json!(folded_form)).unwrap(),
                "{what}：兼容形态与折叠形态不得碰撞（NFC ≠ NFKC）"
            );
            // 完整 Key 字面量冻结 + 与 fixture 交叉核对。
            let key = format!("channel.r_parameter#name={compat_payload}@channel=i:1");
            assert_eq!(validate_point_key(&key), Ok(()), "{what}：{key}");
            let fc = g["key_cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == *ok_id)
                .unwrap_or_else(|| panic!("golden fixture 缺少 key case `{ok_id}`"));
            assert_eq!(fc["key"].as_str().unwrap(), key, "{what}");
        }
    }

    /// **Unicode 数据版本**纳入断言（评审要求：数据版本变化必须明确失败）。
    ///
    /// 规范编码是「(依赖版本, 字符串) → 字节」的函数。依赖版本由 `=` 精确锁定、
    /// 校验和由 `Cargo.lock` 固定；本用例再把**数据版本本身**变成可执行断言，
    /// 并与 golden fixture 的 `unicode_version` 字段交叉核对——依赖升级后若不同步
    /// 更新 fixture，测试会失败，而不是让「golden 声称的 Unicode 版本」与实际实现
    /// 悄悄不一致。升级依赖时**必须**同步更新 fixture 并人工复核全部字面量向量。
    #[test]
    fn nfc_unicode_data_version_is_pinned() {
        let (major, minor, patch) = unicode_normalization::UNICODE_VERSION;
        let actual = format!("{major}.{minor}.{patch}");
        assert_eq!(
            actual, "17.0.0",
            "unicode-normalization 的 Unicode 数据版本已变化：请人工复核 golden 字面量向量"
        );
        // 「Core 只用 NFC」在同一数据版本下还必须与 fixture 声明的版本一致。
        let g = golden();
        assert_eq!(
            g["unicode_version"].as_str().unwrap(),
            actual,
            "golden fixture 的 unicode_version 必须与实际依赖的数据版本一致"
        );
    }

    #[test]
    fn tri_state() {
        assert_eq!(encode_typed_value(&Value::Null).unwrap(), "z:");
        assert_eq!(encode_typed_value(&json!("")).unwrap(), "s:");
        // 缺省 = **无该段**（不得物化默认值）；null = `z:`；空串 = `s:`。三者必须可分。
        let absent = "channel.r_parameter@channel=i:1";
        let explicit_null = "channel.r_parameter#index=z:@channel=i:1";
        let empty_string = "channel.r_parameter#index=s:@channel=i:1";
        for k in [absent, explicit_null, empty_string] {
            assert!(validate_point_key(k).is_ok(), "{k} 应为合法规范 Key");
        }
        assert_ne!(absent, explicit_null);
        assert_ne!(absent, empty_string);
        assert_ne!(explicit_null, empty_string);
    }

    #[test]
    fn object_key_order_and_depth() {
        // 对象键序无关：两条不同书写序的 JSON 文本必须得到同一规范编码。
        // （serde_json 默认 `Map` 为 BTreeMap，输入序在类型层即被抹平；
        //   「唯一规范形式」的可观测后果由下面的正/反例共同证明。）
        let a: Value = serde_json::from_str(r#"{"number":3,"entry":"nc_variable"}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"entry":"nc_variable","number":3}"#).unwrap();
        assert_eq!(
            encode_typed_value(&a).unwrap(),
            encode_typed_value(&b).unwrap()
        );
        assert_eq!(
            encode_typed_value(&a).unwrap(),
            "o:{entry=s:nc_variable,number=i:3}"
        );
        // 递归 + drive_object 示例逐字相等（§4.1.1 K）。
        let nested = json!({"drive_object": {"number": 3, "entry": "nc_variable"}});
        assert_eq!(
            encode_typed_value(&nested).unwrap(),
            "o:{drive_object=o:{entry=s:nc_variable,number=i:3}}"
        );
        assert!(
            validate_point_key(
                "drive.motor.temperature@drive_object=o:{entry=s:nc_variable,number=i:3}"
            )
            .is_ok()
        );
        // 键序未按字节序升序 ⇒ 拒绝（只允许一种规范形式）。
        assert!(
            validate_point_key(
                "drive.motor.temperature@drive_object=o:{number=i:3,entry=s:nc_variable}"
            )
            .is_err()
        );
        // 深度上限 4（根值深度 1）。
        assert!(encode_typed_value(&json!({"a": {"b": {"c": null}}})).is_ok());
        assert_eq!(
            encode_typed_value(&json!({"a": {"b": {"c": {"d": null}}}})).unwrap_err(),
            PointKeyError::DepthExceeded
        );
        assert_eq!(
            encode_typed_value(&json!([[[[[1]]]]])).unwrap_err(),
            PointKeyError::DepthExceeded
        );
        // 对象键名规则（不匹配绝不静默编码）。
        assert!(encode_typed_value(&json!({"Bad": 1})).is_err());
        assert!(encode_typed_value(&json!({"a": 1, "": 2})).is_err());
    }

    #[test]
    fn array_order_preserved() {
        assert_eq!(encode_typed_value(&json!([1, 2])).unwrap(), "a:[i:1,i:2]");
        assert_eq!(encode_typed_value(&json!([2, 1])).unwrap(), "a:[i:2,i:1]");
        assert_ne!(
            encode_typed_value(&json!([1, 2])).unwrap(),
            encode_typed_value(&json!([2, 1])).unwrap()
        );
        assert!(validate_point_key("channel.r_parameter#path=a:[i:1,i:2]@channel=i:1").is_ok());
        assert!(validate_point_key("channel.r_parameter#path=a:[i:2,i:1]@channel=i:1").is_ok());
    }

    #[test]
    fn suffix_grammar() {
        // `~<n>`：n ≥ 2 且无前导零（附加 1）。
        assert!(validate_point_key("native.C.1.21.1.396~2").is_ok());
        assert!(validate_point_key("native.C.1.21.1.396~10").is_ok());
        for bad in [
            "native.C.1.21.1.396~1",
            "native.C.1.21.1.396~0",
            "native.C.1.21.1.396~02",
            "native.C.1.21.1.396~",
            "native.C.1.21.1.396~2~3",
            "native.C.1.21.1.396~x",
        ] {
            assert!(validate_point_key(bad).is_err(), "{bad} 必须被拒");
        }
        // 值中的 `~` 必须编码，否则可伪造后缀（反例 E10）。
        assert_eq!(encode_typed_value(&json!("abc~2")).unwrap(), "s:abc%7E2");
        assert!(validate_point_key("channel.r_parameter#name=s:abc~2@channel=i:1").is_err());
        assert!(validate_point_key("channel.r_parameter#name=s:abc%7E2@channel=i:1").is_ok());
    }

    #[test]
    fn key_grammar_structure() {
        assert_eq!(POINT_KEY_MAX_BYTES, 256);
        assert_eq!(POINT_KEY_MAX_DEPTH, 4);
        // native key：area 段为字母、其余 4 段为十进制。
        assert!(validate_point_key("native.C.1.21.1.396").is_ok());
        assert!(validate_point_key("native.1.1.21.1.396").is_err());
        assert!(validate_point_key("native.C.01.21.1.396").is_err());
        assert!(validate_point_key("native.C.1.21.1").is_err());
        assert!(validate_point_key("native.C.1.21.1.396.7").is_err());
        // 保留前缀：`native.` 开头必须匹配 native_key 形态。
        assert!(validate_point_key("native.foo.bar").is_err());
        // variable_id 语法。
        assert!(validate_point_key("variable.value").is_ok());
        assert!(validate_point_key("variable").is_err());
        assert!(validate_point_key("Channel.R@channel=i:1").is_err());
        // 段内键名按字节序**严格**升序且不重复。
        assert!(validate_point_key("channel.r_parameter#a=i:1,b=i:2@channel=i:1").is_ok());
        assert!(validate_point_key("channel.r_parameter#b=i:1,a=i:2@channel=i:1").is_err());
        assert!(validate_point_key("channel.r_parameter#a=i:1,a=i:2@channel=i:1").is_err());
        assert!(validate_point_key("axis.position.commanded_mcs@axis=i:3,channel=i:1").is_ok());
        assert!(validate_point_key("axis.position.commanded_mcs@channel=i:1,axis=i:3").is_err());
        // 长度上限（不得截断）。
        let long = format!("{}.b", "a".repeat(260));
        assert!(matches!(
            validate_point_key(&long),
            Err(PointKeyError::TooLong { .. })
        ));
    }

    #[test]
    fn e1_e11_golden() {
        let g = golden();
        let mut checked = 0usize;

        let value_cases = g["value_cases"].as_array().unwrap();
        assert!(value_cases.len() >= 11, "E1–E11 向量不得减少");
        for c in value_cases {
            let id = c["id"].as_str().unwrap();
            let expected = c["expected"].as_str().unwrap();
            let got =
                encode_typed_value(&c["input"]).unwrap_or_else(|e| panic!("{id}: 编码失败 {e}"));
            assert_eq!(got, expected, "value case {id}");
            checked += 1;
        }

        for c in g["value_reject_cases"].as_array().unwrap() {
            let id = c["id"].as_str().unwrap();
            let want = c["error"].as_str().unwrap();
            let err = encode_typed_value(&c["input"]).unwrap_err();
            assert_eq!(error_name(&err), want, "value reject case {id}");
            checked += 1;
        }

        let key_cases = g["key_cases"].as_array().unwrap();
        assert!(key_cases.len() >= 40, "Key 向量不得减少");
        for c in key_cases {
            let id = c["id"].as_str().unwrap();
            let key = c["key"].as_str().unwrap();
            let valid = c["valid"].as_bool().unwrap();
            match (valid, validate_point_key(key)) {
                (true, Ok(())) => {}
                (true, Err(e)) => panic!("key case {id} 应通过，实际 {e}：{key}"),
                (false, Ok(())) => panic!("key case {id} 应被拒，实际通过：{key}"),
                (false, Err(e)) => {
                    if let Some(want) = c["error"].as_str() {
                        assert_eq!(error_name(&e), want, "key case {id}（{key}）");
                    }
                }
            }
            checked += 1;
        }
        // 下界：E1–E11 逐条 + 关键结构用例（§6.6 T5 要求 ≥11）。
        assert!(checked >= 11, "golden 向量数 {checked}");
    }

    // -----------------------------------------------------------------------
    // 修订 4 边界：Core 无身份推导
    // -----------------------------------------------------------------------

    #[test]
    fn core_has_no_identity_derivation() {
        // ① 格式合法但**语义身份错误**的 Key 一律放行（Core 不判定身份正确性）。
        for k in [
            "variable.value",                                  // 旧形态（F6），格式合法
            "channel.r_parameter#index=i:999999@channel=i:42", // 与真实变量无关的实例号
            "weird.thing@whatever=i:1",                        // 完全未知的变量与 target
            "drive.motor.temperature@drive_object=o:{entry=s:drive_object,number=i:255}",
        ] {
            assert!(
                validate_point_key(k).is_ok(),
                "Core 必须放行格式合法的 Key（身份判定归 Driver）: {k}"
            );
        }

        // ② `parameters` 完全不透明：Core 不认识其中任何键，也不与 `point_key` 交叉校验。
        let binding = json!({"points": [{
            "kind": "weird_kind",
            "point_key": "variable.value",
            "parameters": {
                "variable_id": "channel.r_parameter",
                "definition_revision": 7,
                "target": {"channel": 1},
                "module": 26,
                "entry": "nc_variable",
                "identity_mode": "user"
            }
        }]});
        assert!(
            validate_point_binding(&binding, "tasks[0].binding.config").is_empty(),
            "Core 不得解释 parameters 内任何键"
        );
        // 参数与 Key 完全不一致也必须放行（「格式合法但身份错误」）。
        let mismatched = json!({"points": [{
            "kind": "nck_variable",
            "point_key": "native.C.1.21.1.396",
            "parameters": {"variable_id": "channel.program.status", "target": {"channel": 1}}
        }]});
        assert!(validate_point_binding(&mismatched, "root").is_empty());

        // ③ 无 Point Kind 白名单：任意**非空** kind 一律放行（P9）。
        for kind in ["nck_variable", "nck_native", "weird_kind", "S7", "🎛"] {
            let b = json!({"points": [{"kind": kind, "point_key": "a.b", "parameters": {}}]});
            assert!(
                validate_point_binding(&b, "root").is_empty(),
                "Core 不得建立 Point Kind 白名单: {kind}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 信封层（§2.4 Core 校验清单）
    // -----------------------------------------------------------------------

    #[test]
    fn envelope_layer_codes() {
        let root = "tasks[0].binding.config";
        let c = |v: Value| codes(&validate_point_binding(&v, root));

        // 结构
        assert_eq!(c(json!("x")), vec!["INVALID_STRUCTURE"]);
        assert_eq!(c(json!({})), vec!["INVALID_STRUCTURE"]);
        assert_eq!(c(json!({"points": {}})), vec!["INVALID_STRUCTURE"]);
        assert_eq!(c(json!({"points": [1]})), vec!["INVALID_STRUCTURE"]);
        // 信封层未知字段即拒（不得静默丢弃）
        assert_eq!(c(json!({"points": [], "extra": 1})), vec!["UNKNOWN_FIELD"]);
        assert_eq!(
            c(json!({"points": [{"kind": "k", "point_key": "a.b", "parameters": {}, "x": 1}]})),
            vec!["UNKNOWN_FIELD"]
        );
        // 缺字段
        assert_eq!(
            c(json!({"points": [{"point_key": "a.b", "parameters": {}}]})),
            vec!["INVALID_STRUCTURE"]
        );
        assert_eq!(
            c(json!({"points": [{"kind": "k", "parameters": {}}]})),
            vec!["INVALID_STRUCTURE"]
        );
        assert_eq!(
            c(json!({"points": [{"kind": "k", "point_key": "a.b"}]})),
            vec!["INVALID_STRUCTURE"]
        );
        // kind：只判非空
        assert_eq!(
            c(json!({"points": [{"kind": "", "point_key": "a.b", "parameters": {}}]})),
            vec!["EMPTY_POINT_KIND"]
        );
        assert_eq!(
            c(json!({"points": [{"kind": 1, "point_key": "a.b", "parameters": {}}]})),
            vec!["INVALID_STRUCTURE"]
        );
        // point_key：非空 + 通用格式
        assert_eq!(
            c(json!({"points": [{"kind": "k", "point_key": "", "parameters": {}}]})),
            vec!["EMPTY_POINT_KEY"]
        );
        assert_eq!(
            c(json!({"points": [{"kind": "k", "point_key": "not a key", "parameters": {}}]})),
            vec!["INVALID_POINT_KEY"]
        );
        assert_eq!(
            c(json!({"points": [{"kind": "k", "point_key": 1, "parameters": {}}]})),
            vec!["INVALID_STRUCTURE"]
        );
        // parameters：只判是 JSON 对象
        for bad in [json!(null), json!([]), json!(1), json!("x")] {
            assert_eq!(
                c(json!({"points": [{"kind": "k", "point_key": "a.b", "parameters": bad}]})),
                vec!["INVALID_PARAMETERS"]
            );
        }
        // 全合法
        assert!(
            validate_point_binding(
                &json!({"points": [
                    {"kind": "nck_variable", "point_key": "a.b@channel=i:1", "parameters": {"x": [1, {"y": null}]}},
                    {"kind": "nck_native", "point_key": "native.C.1.21.1.396", "parameters": {}}
                ]}),
                root
            )
            .is_empty(),
            "同一任务混合两种 Point Kind 必须放行（§6.6 T1 Core 侧）"
        );
    }

    #[test]
    fn point_binding_denies_unknown_fields() {
        // 信封类型本身也必须拒绝未知字段（`identity` 等预览块**不得**进入持久化信封）。
        assert!(PointBinding::from_json(&json!({"points": [], "identity": {}})).is_err());
        assert!(
            PointBinding::from_json(&json!({"points": [
                {"kind": "k", "point_key": "a.b", "parameters": {}, "identity": {}}
            ]}))
            .is_err()
        );
        assert!(PointBinding::from_json(&json!({"points": [], "extra": 1})).is_err());
        let ok = PointBinding::from_json(&json!({"points": [
            {"kind": "k", "point_key": "a.b", "parameters": {"nested": {"a": 1}}}
        ]}))
        .unwrap();
        assert_eq!(ok.points.len(), 1);
        assert_eq!(ok.points[0].kind, "k");
        assert_eq!(ok.points[0].point_key, "a.b");
        assert_eq!(ok.points[0].parameters["nested"]["a"], json!(1));
        // `parse_point_binding` 在校验通过后必然反序列化成功。
        let (issues, parsed) = parse_point_binding(
            &json!({"points": [{"kind": "k", "point_key": "a.b", "parameters": {}}]}),
            "root",
        );
        assert!(issues.is_empty());
        assert!(parsed.is_some());
    }

    #[test]
    fn endpoint_wide_unique_across_kinds() {
        // 同 kind 内跨 Task 重复 ⇒ 拒绝
        let dup = ensure_endpoint_point_keys_unique(
            &[("a.b@channel=i:1", "tasks[1].binding.config.points[0]")],
            &[("a.b@channel=i:1", "tasks[0].binding.config.points[0]")],
        );
        assert_eq!(codes(&dup), vec!["DUPLICATE_POINT_KEY"]);
        // 同一批内重复（第 2 次出现才报）
        let dup2 = ensure_endpoint_point_keys_unique(&[("k", "p0"), ("k", "p1")], &[]);
        assert_eq!(codes(&dup2), vec!["DUPLICATE_POINT_KEY"]);
        assert_eq!(dup2[0].path, "p1");
        // 不重复 ⇒ 放行
        assert!(ensure_endpoint_point_keys_unique(&[("k1", "p0"), ("k2", "p1")], &[]).is_empty());
        // 不同 Endpoint 的同名 key 不冲突：本函数按 Endpoint 调用，两次独立调用各自放行。
        assert!(ensure_endpoint_point_keys_unique(&[("k", "p0")], &[]).is_empty());
        assert!(ensure_endpoint_point_keys_unique(&[("k", "p0")], &[]).is_empty());
    }
}
