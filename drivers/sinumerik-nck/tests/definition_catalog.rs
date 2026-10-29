//! 定义目录（N6）：完整性 / revision 不可变 / digest / 语义状态 / `golden_ref` 双射 /
//! **A 项静态判据** / **C2 禁自证循环**。
//!
//! 覆盖边界文档 §5.1 矩阵的 **#1–#12 与 #63**。本目标只断言**数据与契约**，
//! 编译器行为放在 `compiler_identity.rs`（失败原因必须能在单一目标里定位）。
//!
//! 数据来源全部是**内嵌/冻结文件**（`include_str!`）：定义目录、摘要清单、
//! 62 条冻结 wire 向量、模板 id 清单。没有任何网络/设备访问。

use std::collections::{BTreeMap, BTreeSet};

use mesa_driver_sinumerik_nck::{
    DRIVE_OBJECT_NAME, DRIVE_OBJECT_NAME_M130, DefinitionCatalog, DefinitionScope, DriveEntry,
    NativeFieldSource, PROBES_828D, SemanticsStatus, SourceKind, TargetKind, TargetRef,
    VariableDefinition,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 定义数据（发行文件）。
const R1_JSON: &str = include_str!("../definitions/828d/r1.json");
/// 摘要清单（与 r1.json 同目录）。
const DIGESTS_LOCK: &str = include_str!("../definitions/828d/DIGESTS.lock");
/// 62 条冻结 wire 向量（**唯一**一份真值，禁止复制）。
const WIRE_GOLDEN: &str = include_str!("fixtures/legacy-828d-wire.json");
/// 模板 id 清单来源（只读对照，不参与定义生成）。
const PRESETS_828D: &str = include_str!("../presets/828d.json");

/// C2 禁自证循环的**源码级**检测对象（①）。
const COMPILER_SRC: &str = include_str!("../src/compiler.rs");
const DEFINITION_SRC: &str = include_str!("../src/definition.rs");
const SEMANTIC_EQUIVALENCE_SRC: &str = include_str!("semantic_equivalence.rs");

fn catalog() -> DefinitionCatalog {
    DefinitionCatalog::embedded().expect("内嵌定义目录必须自检通过")
}

fn golden_entries() -> Vec<Value> {
    let doc: Value = serde_json::from_str(WIRE_GOLDEN).unwrap();
    doc["entries"].as_array().unwrap().clone()
}

fn template_ids() -> BTreeSet<String> {
    let doc: Value = serde_json::from_str(PRESETS_828D).unwrap();
    doc["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

/// #1：恰 42 条定义、revision 全为 1、`(variable_id, revision)` 唯一；A16+B20+C6。
#[test]
fn catalog_has_42_definitions_at_revision_1() {
    let cat = catalog();
    assert_eq!(cat.len(), 42, "A 16 + B 20 + C 6 = 42");
    let defs: Vec<&VariableDefinition> = cat.iter().collect();
    assert!(defs.iter().all(|d| d.revision == 1));
    assert!(defs.iter().all(|d| d.family == "828d"));
    let ids: BTreeSet<&str> = defs.iter().map(|d| d.variable_id.as_str()).collect();
    assert_eq!(ids.len(), 42, "(variable_id, revision) 必须唯一");
    // 分类计数：A/C 走单条 golden；B 走两条（nc_variable + drive_object）。
    let singles = defs.iter().filter(|d| d.golden_ref.len() == 1).count();
    let doubles = defs.iter().filter(|d| d.golden_ref.len() == 2).count();
    assert_eq!(singles, 22, "A 16 + C 6");
    assert_eq!(doubles, 20, "B 20（两条入口各一条向量）");
    assert_eq!(
        defs.iter()
            .filter(|d| d.scope == DefinitionScope::Identity)
            .count(),
        6,
        "身份探针 6 条"
    );
}

/// #2：ID 形状 + 保留前缀。
#[test]
fn variable_id_shape_and_reserved_prefix() {
    for d in catalog().iter() {
        let id = &d.variable_id;
        assert!(id.contains('.'), "{id} 必须至少两段");
        assert!(
            id.split('.')
                .next()
                .unwrap()
                .chars()
                .next()
                .unwrap()
                .is_ascii_lowercase(),
            "{id} 首段必须以小写字母开头"
        );
        for seg in id.split('.') {
            assert!(
                !seg.is_empty()
                    && seg
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{id} 段 `{seg}` 越界字符"
            );
        }
        assert_ne!(id, "native");
        assert!(!id.starts_with("native."), "{id} 不得使用保留前缀");
    }
}

/// #3：`semantics_status` 决定 ID 形态（防语义虚构）。
///
/// NOTE（与边界文档矩阵 #3 的**已知数字出入**，如实记录）：矩阵行写
/// 「19 条 `parameter_identity`」，但同一行又写「`r0031`/`r0033` = `candidate`」。
/// 按 §2.5.1 表 B 的**权威数据**，`parameter_identity` 实为 **16** 条
/// （20 条驱动参数 − 2 verified − 2 candidate）。本测试按表 B 数据断言，
/// 并把差异记录在交付报告中。
#[test]
fn semantics_status_governs_id_shape() {
    let cat = catalog();
    let pi: Vec<&VariableDefinition> = cat
        .iter()
        .filter(|d| d.semantics_status == SemanticsStatus::ParameterIdentity)
        .collect();
    assert_eq!(pi.len(), 16, "参数身份型定义数（表 B 权威数据）");
    for d in &pi {
        assert!(
            d.variable_id.starts_with("drive.parameter."),
            "无语义证据者必须用参数身份 ID：{}",
            d.variable_id
        );
    }
    let status = |id: &str| cat.lookup(id, 1).unwrap().semantics_status;
    assert_eq!(status("drive.parameter.r0031"), SemanticsStatus::Candidate);
    assert_eq!(status("drive.parameter.r0033"), SemanticsStatus::Candidate);
    assert_eq!(status("drive.motor.temperature"), SemanticsStatus::Verified);
    assert_eq!(status("drive.object.name"), SemanticsStatus::Verified);
    // 不得出现编造的物理语义名（`drive.motor.` 只允许已批准的 r0035 那条）。
    let motor_ids: Vec<&str> = cat
        .iter()
        .filter(|d| d.variable_id.starts_with("drive.motor."))
        .map(|d| d.variable_id.as_str())
        .collect();
    assert_eq!(motor_ids, vec!["drive.motor.temperature"]);
    for banned in ["drive.current", "drive.torque", "drive.load", "drive.axis"] {
        assert!(
            !cat.iter().any(|d| d.variable_id.starts_with(banned)),
            "不得编造 `{banned}.*` 语义名"
        );
    }
    // candidate 的展示文案必须显式标注候选（不宣称已确认工程量）。
    for id in ["drive.parameter.r0031", "drive.parameter.r0033"] {
        assert!(cat.lookup(id, 1).unwrap().label.zh.contains("候选"), "{id}");
    }
}

/// #4：同一 `variable_id` 的所有 revision 的 `identity_template`/`identity_keys`
/// **逐字相同**（改模板 = 新 `variable_id`）。
///
/// 当前发行数据只有 rev 1；为了真正验证这条**跨 revision** 不变量，这里用内存目录
/// 构造 rev 2（内容除 revision 外完全相同）并断言身份契约逐字不变；再构造一个
/// 「rev 2 身份契约被改」的目录，断言装载期直接拒绝。
#[test]
fn identity_template_and_keys_identical_across_all_revisions() {
    let base = catalog();
    let mut r2 = base.lookup("channel.r_parameter", 1).unwrap().clone();
    r2.revision = 2;
    let two = DefinitionCatalog::from_definitions(vec![
        base.lookup("channel.r_parameter", 1).unwrap().clone(),
        r2,
    ])
    .expect("同身份契约的 rev2 必须合法");
    assert_eq!(two.available_revisions("channel.r_parameter"), vec![1, 2]);
    let a = two.lookup("channel.r_parameter", 1).unwrap();
    let b = two.lookup("channel.r_parameter", 2).unwrap();
    assert_eq!(a.identity_template, b.identity_template);
    assert_eq!(a.identity_keys, b.identity_keys);

    // 反例：rev2 改了身份键集合的顺序（集合相同、字节不同）⇒ 装载期拒绝。
    let mut bad = base.lookup("channel.r_parameter", 1).unwrap().clone();
    bad.revision = 2;
    bad.identity_keys.reverse();
    let err = DefinitionCatalog::from_definitions(vec![
        base.lookup("channel.r_parameter", 1).unwrap().clone(),
        bad,
    ])
    .unwrap_err();
    assert!(
        err.to_string().contains("新 variable_id"),
        "跨 revision 改身份契约必须报「新 variable_id」：{err}"
    );
}

/// 覆盖闭包 `cover(def)`（§5.2 A-1 的机械形式）。
fn cover(def: &VariableDefinition) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for source in def.addressing.sources() {
        match source {
            NativeFieldSource::Fixed { .. } => {}
            NativeFieldSource::Target { kind } => {
                out.insert(kind.as_str().to_string());
            }
            NativeFieldSource::Argument { key } | NativeFieldSource::ArgumentOffset { key, .. } => {
                out.insert(key.clone());
            }
            NativeFieldSource::DriveEntryModule => {
                // 递归对象编码含 entry（§5.2 A-1 末行）。
                out.insert(TargetKind::DriveObject.as_str().to_string());
            }
        }
    }
    out
}

/// #5：**A-1 静态判据**——影响寻址的来源必须全部进入身份（A-1a/A-1c/A-1d）。
#[test]
fn identity_keys_cover_every_addressing_source() {
    for d in catalog().iter() {
        let keys: BTreeSet<String> = d.identity_keys.iter().map(|k| k.path.clone()).collect();
        for needed in cover(d) {
            assert!(
                keys.contains(&needed),
                "{}：寻址来源 `{needed}` 未进入身份（A-1a）",
                d.variable_id
            );
        }
        // A-1d：必填 Target 必须进身份。
        for kind in &d.target_required {
            assert!(
                keys.contains(kind.as_str()),
                "{}：必填 target `{}` 未进入身份（A-1d）",
                d.variable_id,
                kind.as_str()
            );
        }
        // A-1c 的推论：`Fixed` 字段**不**出现在身份键里（同变量内恒定 ⇒ 不区分点位）。
        for source in d.addressing.sources() {
            if let NativeFieldSource::Fixed { .. } = source {
                // 固定来源不产生任何键名，故无可断言项；此处仅确认覆盖闭包不含它。
            }
        }
        assert!(
            !keys.contains("count") && !keys.contains("decode") && !keys.contains("module"),
            "{}：身份键不得含非寻址来源",
            d.variable_id
        );
    }
}

/// #6：模板占位符集合 == `identity_keys` 叶路径集合；占位符按字节序升序、无重复。
#[test]
fn identity_template_placeholders_equal_identity_keys_leaves() {
    for d in catalog().iter() {
        let t = &d.identity_template;
        assert!(t.starts_with(&d.variable_id), "{}", d.variable_id);
        // 占位符：`{path}`，按出现顺序提取。
        let mut placeholders = Vec::new();
        let mut rest = &t[d.variable_id.len()..];
        while let Some(open) = rest.find('{') {
            let close = rest[open..].find('}').expect("占位符必须闭合") + open;
            placeholders.push(rest[open + 1..close].to_string());
            rest = &rest[close + 1..];
        }
        let mut sorted = placeholders.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            placeholders.len(),
            "{}：占位符重复",
            d.variable_id
        );
        let mut from_keys: Vec<String> = d.identity_keys.iter().map(|k| k.path.clone()).collect();
        from_keys.sort();
        assert_eq!(
            sorted, from_keys,
            "{}：占位符 != 身份键叶路径",
            d.variable_id
        );
        // 段内键名按字节序严格升序（模板顺序即规范顺序，消费方不得重排）。
        for segment in t.split(['#', '@']).skip(1) {
            let keys: Vec<&str> = segment
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|item| item.split('=').next().unwrap())
                .collect();
            for pair in keys.windows(2) {
                assert!(
                    pair[0].as_bytes() < pair[1].as_bytes(),
                    "{}：段内键名必须严格升序，实际 {:?}",
                    d.variable_id,
                    keys
                );
            }
        }
    }
}

/// #7：`golden_ref` 与 62 条冻结向量**满射 + 单射 + 双射**，且与 62 个模板 id 一对一。
#[test]
fn golden_ref_is_a_bijection_with_62_wire_vectors() {
    let cat = catalog();
    let golden = golden_entries();
    assert_eq!(golden.len(), 62, "冻结向量不得减少");
    let vector_keys: BTreeSet<(String, String, String, String)> = golden
        .iter()
        .map(|e| {
            (
                e["area"].as_str().unwrap().into(),
                e["block"].as_str().unwrap().into(),
                e["variable"].as_str().unwrap().into(),
                e["entry"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(vector_keys.len(), 62, "冻结向量的四元组必须唯一");

    // 满射 + 双射：每条定义的每个 golden_ref 唯一命中一条冻结向量，且不被两条定义共享。
    type VectorKey = (String, String, String, String);
    let mut bound: BTreeMap<VectorKey, Vec<(String, String)>> = BTreeMap::new();
    let mut total_refs = 0usize;
    for d in cat.iter() {
        let mut entries: Vec<DriveEntry> = Vec::new();
        for g in &d.golden_ref {
            total_refs += 1;
            let key = (
                g.area.clone(),
                g.block.clone(),
                g.variable.clone(),
                g.entry.as_str().to_string(),
            );
            assert!(
                vector_keys.contains(&key),
                "{}：golden_ref {key:?} 不在冻结向量中",
                d.variable_id
            );
            bound
                .entry(key)
                .or_default()
                .push((d.variable_id.clone(), g.entry.as_str().to_string()));
            entries.push(g.entry);
        }
        entries.sort();
        entries.dedup();
        assert_eq!(
            entries.len(),
            d.golden_ref.len(),
            "{}：同一 entry 不得重复绑定同一条 golden",
            d.variable_id
        );
        // 编译器查询必须恰好命中一条定义。
        for g in &d.golden_ref {
            let hits = cat.find_by_golden_ref(g);
            assert_eq!(
                hits.len(),
                1,
                "{}：golden_ref {g:?} 命中 {} 条定义",
                d.variable_id,
                hits.len()
            );
            assert_eq!(hits[0].variable_id, d.variable_id);
        }
    }
    assert_eq!(total_refs, 62, "A 16×1 + B 20×2 + C 6×1 = 62");

    // 单射：62 条冻结向量每一条都被**至少一个** golden_ref 命中，未命中数必须为 0。
    let uncovered: Vec<_> = vector_keys
        .iter()
        .filter(|k| !bound.contains_key(*k))
        .collect();
    assert!(uncovered.is_empty(), "未被覆盖的冻结向量：{uncovered:?}");

    // 双射：每条冻结向量恰被一条定义绑定；`drive_object` 与 `nc_variable` 绑定不同条目。
    for (key, owners) in &bound {
        assert_eq!(owners.len(), 1, "{key:?} 被多条定义绑定：{owners:?}");
    }
    let drive_vectors: BTreeSet<_> = bound
        .iter()
        .filter(|(k, _)| k.3 == "drive_object")
        .map(|(k, _)| k.clone())
        .collect();
    let nc_vectors: BTreeSet<_> = bound
        .iter()
        .filter(|(k, _)| k.3 == "nc_variable")
        .map(|(k, _)| k.clone())
        .collect();
    assert_eq!(drive_vectors.len(), 20);
    assert_eq!(nc_vectors.len(), 42);
    assert!(drive_vectors.is_disjoint(&nc_vectors));

    // 模板 id ↔ golden_ref 一对一（模板文件保持原生字段，逐字不动）。
    let tids = template_ids();
    let built: BTreeSet<String> = bound
        .keys()
        .map(|(a, b, v, e)| format!("828d.{a}.{b}.{v}.{e}"))
        .collect();
    assert_eq!(built.len(), 62);
    assert_eq!(built, tids, "62 个 GoldenRef 必须与 62 个模板 id 一对一");
}

/// #8：6 条 `scope=Identity` 的定义与 `identity_spec.rs` 的探针常量逐字段相等。
#[test]
fn scope_identity_definitions_match_identity_spec_constants() {
    let cat = catalog();
    // 定义 variable_id ↔ 探针名 的固定对应（顺序即定义文件顺序）。
    let cases: [(&str, &str); 6] = [
        ("axis.name", "machine_axis_name"),
        ("axis.ctrlout.module_number", "axis_ctrlout_module_nr"),
        ("axis.encoder.module_number", "axis_enc_module_nr"),
        ("axis.ctrlout.type", "axis_ctrlout_type"),
        ("channel.machine_axis_assignment", "channel_machax_used"),
        ("axis.spindle_assignment", "axis_spind_assign"),
    ];
    for (id, probe_name) in cases {
        let d = cat.lookup(id, 1).unwrap();
        assert_eq!(d.scope, DefinitionScope::Identity);
        let probe = PROBES_828D
            .iter()
            .find(|p| p.name == probe_name)
            .unwrap_or_else(|| panic!("缺探针 {probe_name}"));
        assert_eq!(d.addressing.area, probe.area, "{id} area 漂移");
        assert!(matches!(
            d.addressing.module,
            NativeFieldSource::Fixed { value } if value == u32::from(probe.module)
        ));
        assert_eq!(d.addressing.column, probe.column, "{id} column 漂移");
        assert_eq!(d.decode, probe.decode, "{id} decode 漂移");
        assert_eq!(d.count, probe.count, "{id} count 漂移");
        assert_eq!(
            d.transport_size, probe.transport_size,
            "{id} transport 漂移"
        );
        assert_eq!(d.unit_mode, probe.unit_mode, "{id} unit_mode 漂移");
    }
    // 其余 36 条是采集面（不因身份探针而进入用户点位面）。
    assert_eq!(
        cat.iter()
            .filter(|d| d.scope == DefinitionScope::Collect)
            .count(),
        36
    );
}

/// #9：`DriveEntry::module()` 是 26/130 的唯一出处，且与 `identity_spec.rs` 常量一致。
#[test]
fn drive_entry_module_matches_identity_spec_constants() {
    assert_eq!(DriveEntry::NcVariable.module(), 26);
    assert_eq!(DriveEntry::DriveObject.module(), 130);
    assert_ne!(
        DriveEntry::NcVariable.module(),
        DriveEntry::DriveObject.module(),
        "两条入口**不允许折叠**"
    );
    assert_eq!(DRIVE_OBJECT_NAME.module, 26);
    assert_eq!(DRIVE_OBJECT_NAME_M130.module, 130);
    assert_eq!(DriveEntry::NcVariable.module(), DRIVE_OBJECT_NAME.module);
    assert_eq!(
        DriveEntry::DriveObject.module(),
        DRIVE_OBJECT_NAME_M130.module
    );
    // 定义目录里 B 组的 module 必须**声明为** DriveEntryModule（唯一映射点）。
    let cat = catalog();
    for d in cat.iter().filter(|d| d.addressing.area.letter() == 'V') {
        assert!(
            matches!(d.addressing.module, NativeFieldSource::DriveEntryModule),
            "{}：V 区定义的 module 必须来自入口身份",
            d.variable_id
        );
    }
    // 同 column 的两条入口在冻结向量中必须 module 不同、wire 不同（G4 的数据侧）。
    let golden = golden_entries();
    let mut pairs = 0;
    for e in &golden {
        if e["entry"] != "drive_object" {
            continue;
        }
        let nc = golden
            .iter()
            .find(|x| {
                x["area"] == e["area"]
                    && x["block"] == e["block"]
                    && x["variable"] == e["variable"]
                    && x["entry"] == "nc_variable"
            })
            .expect("同参数必须有 NC 变量入口");
        assert_eq!(nc["column"], e["column"]);
        assert_ne!(nc["module"], e["module"]);
        assert_ne!(nc["wire_hex"], e["wire_hex"]);
        pairs += 1;
    }
    assert_eq!(pairs, 20);
}

/// 装载期自检：`digest` 必须等于 canonical JSON 的 SHA-256，且与 `DIGESTS.lock` 一致。
#[test]
fn digest_matches_lock_and_is_canonical_json_sha256() {
    let cat = catalog();
    let lock: Value = serde_json::from_str(DIGESTS_LOCK).unwrap();
    let entries = lock["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 42, "Lock 必须逐条登记已发布定义");
    let mut locked: BTreeMap<(String, u32), (String, String)> = BTreeMap::new();
    for e in entries {
        let key = (
            e["variable_id"].as_str().unwrap().to_string(),
            e["revision"].as_u64().unwrap() as u32,
        );
        assert!(
            locked
                .insert(
                    key,
                    (
                        e["digest"].as_str().unwrap().to_string(),
                        e["file"].as_str().unwrap().to_string()
                    )
                )
                .is_none(),
            "Lock 条目必须唯一"
        );
    }
    // 目录装载本身已做摘要比对（`verify_digests`），这里再独立复算一次。
    cat.verify_digests().expect("摘要必须与 Lock 一致");
    for d in cat.iter() {
        let key = (d.variable_id.clone(), d.revision);
        let (want, file) = locked.get(&key).expect("Lock 必须登记该定义");
        let got = DefinitionCatalog::compute_digest(d).unwrap();
        assert_eq!(&got, want, "{}@{} 摘要不符", d.variable_id, d.revision);
        assert_eq!(got.len(), 64, "摘要必须是 lowercase hex SHA-256（64 字符）");
        assert!(
            got.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(file, "drivers/sinumerik-nck/definitions/828d/r1.json");
    }
    // 发行文件里**不写** digest（避免「摘要进摘要」的自指）；摘要的唯一权威是 Lock。
    let raw: Value = serde_json::from_str(R1_JSON).unwrap();
    for def in raw["definitions"].as_array().unwrap() {
        assert!(
            def.get("digest").is_none(),
            "{}：发行文件不得自带 digest 字段",
            def["variable_id"]
        );
    }
}

/// #11：canonical 化的五条规则（含**独立**复算与冻结字面量）。
#[test]
fn digest_ignores_its_own_field_and_absent_optionals() {
    let cat = catalog();
    let def = cat.lookup("drive.motor.temperature", 1).unwrap().clone();

    // 规则 3：`digest` 自身不参与。
    let mut with_digest = def.clone();
    with_digest.digest = "0".repeat(64);
    assert_eq!(
        DefinitionCatalog::compute_digest(&with_digest).unwrap(),
        DefinitionCatalog::compute_digest(&def).unwrap()
    );

    // 独立复算：测试内自行实现「递归键排序 + 去 digest → 紧凑 JSON → SHA-256」。
    let mut value = serde_json::to_value(&def).unwrap();
    value.as_object_mut().unwrap().remove("digest");
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                let mut out = serde_json::Map::new();
                for k in keys {
                    if m[k].is_null() {
                        continue;
                    }
                    out.insert(k.clone(), sort(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    let canonical = serde_json::to_string(&sort(&value)).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let expect = format!("{:x}", hasher.finalize());
    assert_eq!(DefinitionCatalog::compute_digest(&def).unwrap(), expect);

    // 冻结字面量（同时冻结 canonical 字符串里的浮点最短往返形态 `-200.0`）。
    assert!(
        canonical.contains(r#""invalid_rule":{"tolerance":0.001,"value":-200.0}"#),
        "浮点必须最短往返且为定点：{canonical}"
    );
    assert_eq!(
        expect, "9a6ed529a0c5ca30c0b1f4564fd30086d7c1e82f82b3dcfd569a2d9ee4f1d1e5",
        "drive.motor.temperature@1 的摘要已冻结（canonical JSON 或数据漂移即失败）"
    );

    // 规则 5：缺省可选字段**不出现**（不写 null）；这里 `unit`/`invalid_rule` 为 None。
    let plain = cat.lookup("drive.parameter.r0021", 1).unwrap();
    let json = serde_json::to_value(plain).unwrap();
    assert!(json.get("unit").is_none());
    assert!(json.get("invalid_rule").is_none());
    assert!(
        !json.to_string().contains("null"),
        "canonical 中不得出现 null"
    );
    // 规则 2：数组**保序** ⇒ 仅 source 顺序不同的两份定义摘要必然不同。
    let mut reordered = plain.clone();
    reordered.source.reverse();
    assert_ne!(
        DefinitionCatalog::compute_digest(plain).unwrap(),
        DefinitionCatalog::compute_digest(&reordered).unwrap(),
        "数组保序：source 顺序是定义语义的一部分"
    );
    // 规则 1：顶层键升序（对象键序无关 ⇒ 手写 JSON 的键序不影响摘要）。
    assert!(canonical.starts_with(r#"{"addressing":"#), "{canonical}");
    assert!(canonical.ends_with(r#""variable_id":"drive.motor.temperature"}"#));
}

/// #12：miss **绝不**回落（三种退化逐条为静态证据）。
#[test]
fn lookup_missing_revision_never_falls_back() {
    let base = catalog();
    // 构造 rev{1,3} 的内存目录：请求 rev 2 时既不能回落 1、也不能回落 3。
    let mut r3 = base.lookup("channel.program.status", 1).unwrap().clone();
    r3.revision = 3;
    let cat = DefinitionCatalog::from_definitions(vec![
        base.lookup("channel.program.status", 1).unwrap().clone(),
        r3,
    ])
    .unwrap();
    assert_eq!(
        cat.available_revisions("channel.program.status"),
        vec![1, 3]
    );
    let err = cat.lookup("channel.program.status", 2).unwrap_err();
    assert_eq!(err.code(), "UNSUPPORTED_DEFINITION_REVISION");
    let msg = err.to_string();
    assert!(msg.contains("channel.program.status"), "{msg}");
    assert!(msg.contains("[1, 3]"), "必须列出可用 revision：{msg}");
    assert!(msg.contains("绝不回落"), "{msg}");
    // 未知 variable_id 是另一个码（不得混用）。
    assert_eq!(
        cat.lookup("no.such.var", 1).unwrap_err().code(),
        "UNKNOWN_VARIABLE"
    );
    // 发行数据只有 rev 1：请求 2 必须失败（不是「取最新」）。
    assert_eq!(
        base.lookup("channel.program.status", 2).unwrap_err().code(),
        "UNSUPPORTED_DEFINITION_REVISION"
    );
}

/// #63：**C2=P1 禁自证循环**——新定义目录不是旧模板的派生物。
///
/// ① **源码级**：`compiler.rs` / `definition.rs` / `semantic_equivalence.rs` 对
///    模板注册表、模板类型、内嵌模板常量与模板路径**零引用**；
/// ② **来源级**：`r1.json` 每条定义的 `source` 只指向手册 / 实测 / 身份探针 /
///    冻结向量，**不得**出现模板文件路径；
/// ③ **无构建期生成**：定义目录内容恰为 {`r1.json`, `DIGESTS.lock`, `r1.meta.json`}
///    ——发行定义只能人工编写、随 PR 提交，不存在 codegen 产物或第二份真值。
///    `r1.meta.json` 是 sidecar（单位/手册位置/索引规则/可用性判定），它**不是**
///    定义的副本：内容与 `r1.json` 不重叠，且通过 `binds_to.definition_blob_sha1`
///    单向绑定到那份定义（改定义不改 sidecar ⇒ 装载期直接失败，见
///    `tests/definition_meta.rs`）。构建期不生成它，只有 CI 校验它
///    （`tools/ci/check_definition_meta.py`）。
///
/// NOTE：矩阵 #63③ 的原文是「构建后 `git status --short -- <definitions>` 为空」。
/// 该断言无法在测试进程内可靠表达（未提交的 PR 工作树本来就与 HEAD 不同），
/// 因此它在**门禁**中实跑并记录（见交付报告），测试内改为等价的机械检查。
#[test]
fn definitions_are_not_derived_from_presets() {
    // ① 源码级零引用。
    for (name, src) in [
        ("compiler.rs", COMPILER_SRC),
        ("definition.rs", DEFINITION_SRC),
        ("semantic_equivalence.rs", SEMANTIC_EQUIVALENCE_SRC),
    ] {
        for needle in [
            "PresetRegistry",
            "PresetTemplate",
            "EMBEDDED_828D",
            "presets/",
            "leaf_node",
        ] {
            assert!(
                !src.contains(needle),
                "{name} 不得引用 `{needle}`（新路径必须独立于旧模板数据）"
            );
        }
    }
    // 语义等价门也不能把模板注册表拉进来（编译期即不可能读模板）。
    for line in SEMANTIC_EQUIVALENCE_SRC.lines() {
        if let Some(rest) = line.trim().strip_prefix("use ") {
            assert!(
                !rest.contains("Preset"),
                "semantic_equivalence.rs 的 use 列表不得含模板类型：{line}"
            );
        }
    }

    // ② 来源级：不得指向任何模板文件。
    for d in catalog().iter() {
        assert!(!d.source.is_empty(), "{} 必须有来源", d.variable_id);
        for s in &d.source {
            assert!(
                !s.reference.contains("presets"),
                "{}：来源 `{}` 指向模板文件（自证循环）",
                d.variable_id,
                s.reference
            );
            assert!(
                !s.reference.to_lowercase().contains("828d.json"),
                "{}：来源 `{}` 指向模板数据文件",
                d.variable_id,
                s.reference
            );
        }
        // 每条定义都必须有一条**冻结向量**来源（wire 真值归属可查）。
        assert!(
            d.source.iter().any(|s| s.kind == SourceKind::FrozenVector),
            "{}：缺 frozen_vector 来源",
            d.variable_id
        );
        for s in &d.source {
            if s.kind == SourceKind::FrozenVector {
                assert_eq!(
                    s.reference, "drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json",
                    "冻结向量来源路径必须唯一"
                );
            }
            if s.kind == SourceKind::IdentityProbe {
                assert!(
                    s.reference
                        .starts_with("drivers/sinumerik-nck/src/identity_spec.rs")
                );
            }
        }
    }

    // ③ 定义目录内容（无 codegen 产物 / 无第二份真值 / 无副本）。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("definitions");
    let mut names: Vec<String> = Vec::new();
    collect_files(&dir, &dir, &mut names);
    names.sort();
    assert_eq!(
        names,
        vec![
            "828d/DIGESTS.lock".to_string(),
            "828d/r1.json".to_string(),
            "828d/r1.meta.json".to_string()
        ],
        "定义目录只允许人工编写的发行文件"
    );
    // 冻结 wire 向量只有一份（禁止复制第二份真值）。
    let tests_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut test_files: Vec<String> = Vec::new();
    collect_files(&tests_dir, &tests_dir, &mut test_files);
    let wire_copies: Vec<&String> = test_files
        .iter()
        .filter(|n| n.contains("wire") && n.ends_with(".json"))
        .collect();
    assert_eq!(
        wire_copies,
        vec![&"fixtures/legacy-828d-wire.json".to_string()],
        "62 条 wire 真值只允许一份"
    );
}

/// 递归收集目录下的文件（仓库相对路径，`/` 分隔）。
fn collect_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("目录必须可读") {
        let path = entry.expect("目录项").path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

/// 最长默认 Key 实测（Core 上限 256 字节**不放宽**，§2.5 待确认项 8）
/// + **A：跨 Core/Driver 身份契约**（PR-B）。
///
/// **证明什么**：Driver 侧生成的身份不仅要「自己看着对」，还必须被 Core 侧的**通用语法校验器**
/// 逐条接受。两侧各自通过各自的测试**并不**蕴含这件事：Driver 可能产出违反通用契约
/// （NFC / 长度 / 字符集 / 段内键序 / 类型标签分隔符）的 Key，而故障只会在**生产保存点位时**
/// 才暴露（Core 拒收），不会在任何一侧的 CI 里暴露。本用例把 **42 条定义 × 两组合法实例
/// = 84 条真实 Driver 身份**全部送进 Core 校验器，把这处缺口关在 CI 里。
///
/// **为什么必须由生产代码提供两侧实现**（测试**不复制**任何身份拼装或 Key 校验算法——
/// 复制一套就等于只测「我自己写的算法」，两侧真实漂移照样漏过）：
/// - Driver 身份生成：`VariableDefinition::canonical_identity`（`src/definition.rs:1091`），
///   与编译器 `src/compiler.rs:398`、预填 `src/presets.rs:250` 调用的是**同一函数**；
/// - Driver 侧合法实例闸门：`TargetRef::validate`（`src/targets.rs:303`）与
///   `VariableDefinition::validate_arguments`（`src/definition.rs:997`），
///   即 `src/compiler.rs:383` / `src/compiler.rs:385` 使用的同两条校验
///   （否则「合法实例」只是测试的自称）；
/// - Core Key 校验：`mesa_core_types::validate_point_key`（`crates/core-types/src/point.rs:413`）。
///
/// 样本取「最小合法实例 + 每个 argument 的上界」：数字段的最长形态出现在多位实例号
/// （如 255）与多位 argument 值，因此额外取 `255` 作为实例号样本，确保测到的是
/// **可能的最长形态**而非单一示例。
#[test]
fn default_keys_stay_within_core_limit() {
    let cat = catalog();
    let mut keys: Vec<(String, String)> = Vec::new();
    for d in cat.iter() {
        for value in [1u64, 255] {
            let mut target = serde_json::Map::new();
            for kind in &d.targets {
                match kind {
                    TargetKind::DriveObject => {
                        target.insert(
                            "drive_object".into(),
                            serde_json::json!({"number": value, "entry": "drive_object"}),
                        );
                    }
                    other => {
                        target.insert(other.as_str().into(), serde_json::json!(value));
                    }
                }
            }
            let mut arguments = serde_json::Map::new();
            for spec in &d.arguments {
                // 取规格允许的**最长数字形态**（上界）。
                arguments.insert(
                    spec.key.clone(),
                    serde_json::json!(spec.max.unwrap_or(0).max(0)),
                );
            }
            let target = TargetRef::from_json(&Value::Object(target)).unwrap();
            // 这两条闸门必须与 Compiler 的 D7/D8 同源：样本若本身不合法，
            // 「84 条真实身份」就只是测试的自称（`src/compiler.rs:383/385`）。
            target
                .validate(d)
                .unwrap_or_else(|e| panic!("{}：样本 target 不合法（D7）：{e}", d.variable_id));
            d.validate_arguments(&arguments)
                .unwrap_or_else(|e| panic!("{}：样本 arguments 不合法（D8）：{e}", d.variable_id));
            let key = d.canonical_identity(&target, &arguments).unwrap();
            keys.push((d.variable_id.clone(), key));
        }
    }
    assert_eq!(keys.len(), 84, "42 条定义 × 两组合法实例 = 84 条真实身份");

    let mut longest = (0usize, String::new());
    for (variable_id, key) in &keys {
        // ① Core 通用语法（非空 / NFC / 字节长 / 字符集 / 段内键名升序 / 规范类型标签编码）
        //    必须逐条接受——这正是「Driver 生成 ⇒ Core 接受」的契约本身。
        mesa_core_types::validate_point_key(key).unwrap_or_else(|e| {
            panic!("{variable_id}：Driver 生成的身份被 Core 拒绝：{e}（key = {key}）")
        });
        // ② 编码后字节数必须 ≤ Core 上限（逐条，不只看最长的那条）。
        assert!(
            key.len() <= 256,
            "{variable_id}：{key} 编码后 {} 字节超过 Core 上限 256",
            key.len()
        );
        // ③ 类型标签分隔符**恒为** `:`；`=i=` / `=s=` 是错误形态，84 条里一条都不许出现。
        assert!(
            !key.contains("=i="),
            "{variable_id}：{key} 出现错误分隔符 `=i=`（类型标签分隔符恒为 `:`）"
        );
        assert!(
            !key.contains("=s="),
            "{variable_id}：{key} 出现错误分隔符 `=s=`（类型标签分隔符恒为 `:`）"
        );
        if key.len() > longest.0 {
            longest = (key.len(), key.clone());
        }
    }

    println!("最长默认 Key = {} 字节：{}", longest.0, longest.1);
    // ④ 最长 Key 的**规范字面量**冻结：定义数据或身份拼装一旦漂移，这里立刻失败。
    const LONGEST_KEY: &str =
        "drive.motor.temperature@drive_object=o:{entry=s:drive_object,number=i:255}";
    assert_eq!(longest.1, LONGEST_KEY, "最长 Key 的规范字面量已冻结");
    assert_eq!(
        longest.0, 74,
        "最长 Key 必须恰为 74 字节（与 Core 上限 256 相互独立的第二条判据）"
    );
    assert_eq!(LONGEST_KEY.len(), 74, "冻结字面量自身必须为 74 字节");
    assert!(
        longest.0 >= 60,
        "若最长 Key 突然变短，说明样本或定义发生了非预期变化：{}",
        longest.1
    );

    // ⑤「明确拒绝错误分隔符」：不能只断言「我们没写出来」——把最长 Key 的同形反例
    //    （`number=i=255` / `entry=s=drive_object`）交给**同一个生产校验器**，必须报错。
    assert!(mesa_core_types::validate_point_key(LONGEST_KEY).is_ok());
    let wrong_separator = [
        "drive.motor.temperature@drive_object=o:{entry=s:drive_object,number=i=255}",
        "drive.motor.temperature@drive_object=o:{entry=s=drive_object,number=i:255}",
    ];
    for bad in wrong_separator {
        let err = mesa_core_types::validate_point_key(bad).expect_err(&format!(
            "含错误分隔符（`=i=` / `=s=`）的 Key 必须被 Core 拒绝：{bad}"
        ));
        println!("错误分隔符反例被拒：{bad} ⇒ {err}");
    }
}

/// B：**Lock 摘要被篡改 ⇒ 装载失败，且失败被映射为 `DEFINITION_DIGEST_MISMATCH`**；
/// 同时证明**仓库内正式 `DIGESTS.lock` 未被本次测试改动**。
///
/// **证明什么**：摘要校验必须 fail-closed，并且失败要落到**稳定的对外错误码**
/// （不是「随便一个 JSON 错误」）。若装载路径吞掉 `DigestMismatch`、或把它错报成
/// `Json` / `LockJson`，调用方（状态机 / CLI）就无法区分「定义被篡改」与「文件格式坏」，
/// 而这正是 §3.5 A14 要求 21 个错误码逐条可映射的意义。
///
/// **为什么不触碰正式 Lock，以及为什么校验必须走生产路径**：
/// - 篡改只发生在**内存副本**上：把 `include_str!` 读入的官方 Lock 文本解析为
///   `serde_json::Value` → 翻转某一条 `digest` 的一位十六进制 → 重新序列化成字符串；
///   仓库内的 `definitions/828d/DIGESTS.lock` **不写、不删、不改名**（测试用只读
///   `std::fs::read` + SHA-256 读回比对证明，`git status` 亦为空）；
/// - 校验走**唯一**的真实装载路径 `DefinitionCatalog::load`（`src/definition.rs:381`，
///   `embedded()` 只是它的封装），内部由 `verify_digests`（`src/definition.rs:546`）
///   产出 `CatalogError::DigestMismatch`，对外 code 由 `CatalogError::code`
///   （`src/definition.rs:332`）映射。测试**不复制**摘要算法，也不另写一套装载校验。
#[test]
fn catalog_error_maps_to_definition_digest_mismatch() {
    // 0) 正式 Lock 的基线指纹：磁盘读回 ≡ 编译期内嵌副本（两处互证）。
    let lock_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("definitions/828d/DIGESTS.lock");
    let baseline = sha256_hex(&std::fs::read(&lock_path).expect("正式 DIGESTS.lock 必须存在"));
    assert_eq!(
        baseline,
        sha256_hex(DIGESTS_LOCK.as_bytes()),
        "磁盘上的正式 DIGESTS.lock 必须与编译期内嵌副本逐字一致"
    );
    // 对照组：未篡改的官方 Lock 文本必须装载成功（失败只可能来自篡改本身）。
    DefinitionCatalog::load(R1_JSON, DIGESTS_LOCK).expect("未篡改的官方 Lock 必须装载成功");

    // 1) 只在**内存副本**上篡改一条摘要（翻转首个十六进制字符，长度与形状保持 64 位 hex）。
    let official: Value = serde_json::from_str(DIGESTS_LOCK).unwrap();
    let mut doc = official.clone();
    let (victim_id, original) = {
        let first = &doc["entries"].as_array().unwrap()[0];
        (
            first["variable_id"].as_str().unwrap().to_string(),
            first["digest"].as_str().unwrap().to_string(),
        )
    };
    let flipped = flip_first_hex(&original);
    doc["entries"][0]["digest"] = Value::from(flipped.clone());
    // 「篡改」必须可证明：除被翻转的那一条摘要外，副本与正式 Lock **逐字段相同**
    // （条目数不变、其余摘要不变、`file` 指向不变）——否则失败可能来自别处的改动。
    let mut expected_doc = official;
    expected_doc["entries"][0]["digest"] = Value::from(flipped.clone());
    assert_eq!(doc, expected_doc, "篡改必须只发生在被翻转的那一条摘要上");
    assert_eq!(
        doc["entries"].as_array().unwrap().len(),
        42,
        "副本条目数必须与正式 Lock 相同"
    );
    assert_ne!(flipped, original, "被篡改的摘要必须与原值不同");
    assert_eq!(
        flipped.len(),
        64,
        "篡改后的摘要仍须是 64 位 hex（只变值、不变形状）"
    );
    let tampered = serde_json::to_string_pretty(&doc).unwrap();

    // 2) 真实装载路径：必须失败。
    let err = DefinitionCatalog::load(R1_JSON, &tampered)
        .expect_err("摘要被篡改后装载必须失败（fail-closed）");
    // 3) 失败必须被映射为唯一的目标错误码，且诊断里带上两侧摘要。
    assert_eq!(
        err.code(),
        "DEFINITION_DIGEST_MISMATCH",
        "错误码映射漂移：{err}"
    );
    let msg = err.to_string();
    assert!(msg.contains(&victim_id), "必须点名被篡改的定义：{msg}");
    assert!(
        msg.contains(&flipped),
        "必须报出 Lock 侧（expected）摘要：{msg}"
    );
    assert!(
        msg.contains(&original),
        "必须报出实算（actual）摘要以便比对：{msg}"
    );

    // 4) 正式 Lock 未被修改：磁盘读回指纹不变，且内嵌目录仍能装载 + 自洽。
    assert_eq!(
        sha256_hex(&std::fs::read(&lock_path).expect("正式 DIGESTS.lock 必须仍可读")),
        baseline,
        "正式 DIGESTS.lock 在测试过程中被改动"
    );
    let cat = DefinitionCatalog::embedded().expect("正式内嵌目录必须仍然可装载");
    cat.verify_digests()
        .expect("正式 DIGESTS.lock 与定义目录必须仍然自洽");
}

/// 内容 SHA-256（lowercase hex）——仅用于「读回比对」，不复制任何生产摘要逻辑。
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 翻转首个十六进制字符（保证与原值不同；用于构造 Lock 篡改副本）。
fn flip_first_hex(hex: &str) -> String {
    let mut chars: Vec<char> = hex.chars().collect();
    chars[0] = if chars[0] == 'f' { '0' } else { 'f' };
    chars.into_iter().collect()
}

/// 所有定义都必须可被编译器消费：`targets` / `arguments` / `golden_ref` 自洽。
#[test]
fn every_definition_declares_a_usable_target_and_arguments() {
    for d in catalog().iter() {
        assert!(!d.targets.is_empty(), "{}：targets 不得为空", d.variable_id);
        for kind in &d.target_required {
            assert!(
                d.targets.contains(kind),
                "{}：required ⊆ targets",
                d.variable_id
            );
        }
        // `drive_object` 定义必须走入口身份（26/130 不可折叠）。
        let uses_entry = matches!(d.addressing.module, NativeFieldSource::DriveEntryModule);
        assert_eq!(
            uses_entry,
            d.targets.contains(&TargetKind::DriveObject),
            "{}：DriveObject 与 DriveEntryModule 必须同时出现",
            d.variable_id
        );
        // 参数身份型 ID 一律 `drive.parameter.<参数名>`（如 `r0021` / `p0105`）。
        if d.semantics_status == SemanticsStatus::ParameterIdentity {
            let suffix = d
                .variable_id
                .strip_prefix("drive.parameter.")
                .unwrap_or_else(|| panic!("{}：必须用参数身份 ID", d.variable_id));
            assert!(
                suffix.len() >= 2
                    && suffix.as_bytes()[0].is_ascii_lowercase()
                    && suffix[1..].bytes().all(|b| b.is_ascii_digit()),
                "{}：参数名形态必须为 `<字母><数字>`",
                d.variable_id
            );
        }
        for spec in &d.arguments {
            assert!(spec.required, "{}：本期 argument 一律必填", d.variable_id);
        }
    }
}
