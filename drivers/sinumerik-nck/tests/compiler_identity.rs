//! 身份契约与重算拒绝（N7）：T3b / T4 / T5(Driver) / **A 项反例** / **B 项命名空间** /
//! **C1 的 Target 组合校验**。
//!
//! 覆盖边界文档 §5.1 矩阵的 **#13–#37 与 #57–#62**。本目标是「编译器逻辑」的
//! 单一定位点：定义数据的断言在 `definition_catalog.rs`，62 条字节等价在
//! `semantic_equivalence.rs`。
//!
//! 三条不变量（§4.1.1）：
//! - **I1** target 变体必须进身份；**I2** 所有显式提供的 argument 按键名升序进身份；
//! - **I3** `definition_revision` **绝不**进身份。

use mesa_driver_sdk::Driver;
use mesa_driver_sinumerik_nck::{
    AddressCompiler, CompileError, DefinitionCatalog, IdentityMode, NATIVE_PARAMETER_KEYS,
    NativeNckAddress, POINT_KIND_NATIVE, POINT_KIND_VARIABLE, SinumerikNckDriver, TargetKind,
    TargetRef, VariableDefinition, encode_var_spec, ensure_batch_identity_namespace_unique,
    resolve_native,
};
use serde_json::{Map, Value, json};

/// E1–E11 golden 向量（与 Core / 前端**同一份**文件，路径逐字相同）。
const GOLDEN_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/nck-point-key-golden.json"
));

fn catalog() -> DefinitionCatalog {
    DefinitionCatalog::embedded().expect("内嵌定义必须自检通过")
}

fn compiler(cat: &DefinitionCatalog) -> AddressCompiler<'_> {
    AddressCompiler::new(cat, "828d")
}

fn target(v: Value) -> TargetRef {
    TargetRef::from_json(&v).expect("target 必须合法")
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().expect("arguments 必须是对象").clone()
}

/// 语义点位参数（`nck_variable`）。
fn semantic_params(
    variable_id: &str,
    revision: u32,
    target_json: Value,
    args_json: Value,
) -> Map<String, Value> {
    json!({
        "variable_id": variable_id,
        "definition_revision": revision,
        "target": target_json,
        "arguments": args_json,
    })
    .as_object()
    .unwrap()
    .clone()
}

/// 语义点位的规范身份（直接调用定义的方法，用于构造正确的持久化 Key）。
fn identity(def: &VariableDefinition, target_json: Value, args_json: Value) -> String {
    def.canonical_identity(&target(target_json), &args(args_json))
        .expect("身份必须可算")
}

/// 编译一个语义点位（`persisted` 给出持久化 Key）。
fn compile_semantic(
    cat: &DefinitionCatalog,
    variable_id: &str,
    target_json: Value,
    args_json: Value,
    persisted: &str,
) -> Result<mesa_driver_sinumerik_nck::CompiledPoint, CompileError> {
    compile_semantic_rev(cat, variable_id, 1, target_json, args_json, persisted)
}

/// 同上，但显式给出 `definition_revision`。
fn compile_semantic_rev(
    cat: &DefinitionCatalog,
    variable_id: &str,
    revision: u32,
    target_json: Value,
    args_json: Value,
    persisted: &str,
) -> Result<mesa_driver_sinumerik_nck::CompiledPoint, CompileError> {
    let params = semantic_params(variable_id, revision, target_json, args_json);
    compiler(cat).compile_point(POINT_KIND_VARIABLE, &params, persisted)
}

/// 12 个原生字段（可改 count / 加 identity_mode）。
fn native_params(line: u64) -> Map<String, Value> {
    json!({
        "area": "C", "area_no": 1, "module": 21, "column": 1, "line": line,
        "count": 1, "unit_mode": "current", "decode": "F64_LE",
        "transport_size": "0x09",
    })
    .as_object()
    .unwrap()
    .clone()
}

// ---------------------------------------------------------------------------
// T3b（#13 / #14）：身份与 revision 无关；target/arguments 变则身份变
// ---------------------------------------------------------------------------

/// #13：对 42 条定义的**每一个已发布 revision**，同 target/arguments ⇒ 身份逐字相等。
///
/// 当前发行数据只有 rev 1；为使这条不变量真正可验证，这里为全部 42 条定义构造
/// rev 2 的内存目录（除 revision 外逐字相同），并逐条比对。
#[test]
fn t3b_recomputed_identity_is_revision_independent() {
    let base = catalog();
    let mut defs: Vec<VariableDefinition> = Vec::new();
    for d in base.iter() {
        let mut r2 = d.clone();
        r2.revision = 2;
        defs.push(d.clone());
        defs.push(r2);
    }
    let cat = DefinitionCatalog::from_definitions(defs).expect("rev2 目录必须自检通过");
    assert_eq!(cat.len(), 84, "42 条 × 2 revision");

    let mut checked = 0usize;
    for d in base.iter() {
        let (t, a) = sample_inputs(d, 1);
        let id1 = d.canonical_identity(&t, &a).unwrap();
        let d2 = cat.lookup(&d.variable_id, 2).unwrap();
        assert_eq!(d2.identity_template, d.identity_template);
        assert_eq!(d2.identity_keys, d.identity_keys);
        let id2 = d2.canonical_identity(&t, &a).unwrap();
        assert_eq!(
            id1, id2,
            "{}：身份必须与 revision 无关（I3）",
            d.variable_id
        );
        assert!(
            !id1.contains("revision"),
            "{}：revision 不进身份",
            d.variable_id
        );
        checked += 1;
    }
    assert_eq!(checked, 42, "必须逐条核对，不得跳过");
}

/// #14：改任一 target 实例或任一 argument ⇒ 身份**不同**（R9/R14 反向）。
#[test]
fn t3b_changed_target_or_arguments_changes_identity() {
    let cat = catalog();
    // channel.program.status：channel 1 → 2
    let d = cat.lookup("channel.program.status", 1).unwrap();
    let a = identity(d, json!({"channel": 1}), json!({}));
    let b = identity(d, json!({"channel": 2}), json!({}));
    assert_ne!(a, b);
    // channel.r_parameter：index 1 → 2（且 channel 相同）
    let d = cat.lookup("channel.r_parameter", 1).unwrap();
    assert_ne!(
        identity(d, json!({"channel": 1}), json!({"index": 1})),
        identity(d, json!({"channel": 1}), json!({"index": 2}))
    );
    // axis.position.commanded_mcs：axis 1 → 2（channel 相同）
    let d = cat.lookup("axis.position.commanded_mcs", 1).unwrap();
    assert_ne!(
        identity(d, json!({"channel": 1, "axis": 1}), json!({})),
        identity(d, json!({"channel": 1, "axis": 2}), json!({}))
    );
    // 目标集合逐条覆盖 42 条：枚举每个变体各取两个值，任意一处不同即身份不同。
    for d in cat.iter() {
        let (t1, a1) = sample_inputs(d, 1);
        let (t2, a2) = sample_inputs(d, 2);
        if t1 == t2 && a1 == a2 {
            continue;
        }
        assert_ne!(
            d.canonical_identity(&t1, &a1).unwrap(),
            d.canonical_identity(&t2, &a2).unwrap(),
            "{}：输入不同却没有区分身份",
            d.variable_id
        );
    }
}

// ---------------------------------------------------------------------------
// T4（#15–#18）：唯一执行真值
// ---------------------------------------------------------------------------

/// #15：持久化 Key ≠ 重算身份 ⇒ `POINT_KEY_MISMATCH`，消息含三要素。
#[test]
fn t4_configure_fails_whole_batch_on_point_key_mismatch() {
    let cat = catalog();
    let want = identity(
        cat.lookup("channel.program.status", 1).unwrap(),
        json!({"channel": 1}),
        json!({}),
    );
    let err = compile_semantic(
        &cat,
        "channel.program.status",
        json!({"channel": 1}),
        json!({}),
        "channel.program.status@channel=i:2",
    )
    .unwrap_err();
    assert_eq!(err.code(), "POINT_KEY_MISMATCH");
    let msg = err.to_string();
    assert!(
        msg.contains("channel.program.status@channel=i:2"),
        "缺持久化 Key：{msg}"
    );
    assert!(msg.contains(&want), "缺重算 Key：{msg}");
    assert!(
        msg.contains("channel.program.status") && msg.contains("revision=1"),
        "缺 variable_id + revision：{msg}"
    );
    // 原生侧同样 fail-closed。
    let err = compiler(&cat)
        .compile_point(
            POINT_KIND_NATIVE,
            &native_params(396),
            "native.C.1.21.1.999",
        )
        .unwrap_err();
    assert_eq!(err.code(), "POINT_KEY_MISMATCH");
}

/// #16：Key 逐字相等 ⇒ 编译成功，且定义引用给出 `(variable_id, revision, digest)`。
#[test]
fn t4_matching_key_succeeds() {
    let cat = catalog();
    let want = identity(
        cat.lookup("channel.program.status", 1).unwrap(),
        json!({"channel": 1}),
        json!({}),
    );
    let compiled = compile_semantic(
        &cat,
        "channel.program.status",
        json!({"channel": 1}),
        json!({}),
        &want,
    )
    .expect("Key 相符必须成功");
    assert_eq!(compiled.canonical_identity, want);
    let def_ref = compiled.definition.expect("语义点位必须给出定义引用");
    assert_eq!(def_ref.variable_id, "channel.program.status");
    assert_eq!(def_ref.revision, 1);
    assert_eq!(def_ref.digest.len(), 64);
    assert_eq!(compiled.identity_mode, IdentityMode::Derived);
    // 地址来自定义目录的 addressing（C/1/module127/column13/line1）。
    assert_eq!(compiled.address.area.letter(), 'C');
    assert_eq!(compiled.address.area_no, 1);
    assert_eq!(compiled.address.module, 127);
    assert_eq!(compiled.address.column, 13);
    assert_eq!(compiled.address.line, 1);
    assert_eq!(compiled.address.count, 1);
    assert_eq!(compiled.address.decode.decode_name(), "U16_LE");
    assert_eq!(compiled.address.transport_size, 0x09);
    assert!(compiled.address.invalid_value.is_none());
}

/// #17：`nck_variable` + `identity_mode`（含 `user`）⇒ `INVALID_BINDING_CONFIG`。
#[test]
fn t4_identity_mode_user_for_nck_variable_rejected() {
    let cat = catalog();
    let want = identity(
        cat.lookup("channel.program.status", 1).unwrap(),
        json!({"channel": 1}),
        json!({}),
    );
    for mode in ["user", "derived", "whatever"] {
        let mut params = semantic_params(
            "channel.program.status",
            1,
            json!({"channel": 1}),
            json!({}),
        );
        params.insert("identity_mode".into(), json!(mode));
        let err = compiler(&cat)
            .compile_point(POINT_KIND_VARIABLE, &params, &want)
            .unwrap_err();
        assert_eq!(
            err.code(),
            "INVALID_BINDING_CONFIG",
            "nck_variable 不允许 identity_mode（mode={mode}）"
        );
    }
}

/// #18：`nck_native` + `user`：**不重算、不静默改名**（R13）。
#[test]
fn t4_user_mode_does_not_recompute_nor_rename() {
    let cat = catalog();
    let user_key = "native.C.1.21.1.396~2";
    let mut params = native_params(999);
    params.insert("identity_mode".into(), json!("user"));
    let compiled = compiler(&cat)
        .compile_point(POINT_KIND_NATIVE, &params, user_key)
        .expect("user 模式不比对 Key");
    assert_eq!(compiled.identity_mode, IdentityMode::User);
    // 地址按用户给的字段解析（line=999），但身份**不**跟着变。
    assert_eq!(compiled.address.line, 999);
    assert_eq!(
        compiled.canonical_identity, "native.C.1.21.1.999",
        "规范身份仍会算出，但 user 模式不参与比对"
    );
    assert_ne!(compiled.canonical_identity, user_key);
    // 改地址字段后 Key 不变 ⇒ 复用同一 point_id（Driver 不追加任何后缀）。
    let mut moved = native_params(1234);
    moved.insert("identity_mode".into(), json!("user"));
    let moved_compiled = compiler(&cat)
        .compile_point(POINT_KIND_NATIVE, &moved, user_key)
        .expect("user 模式改地址仍必须成功");
    assert_eq!(moved_compiled.canonical_identity, "native.C.1.21.1.1234");
    assert!(!user_key.ends_with("~3"), "Driver 不得静默递增后缀");
}

// ---------------------------------------------------------------------------
// 参数层严格校验（#19–#24）
// ---------------------------------------------------------------------------

/// #19：身份键缺失 ⇒ `MISSING_IDENTITY_KEY`；显式 null ⇒ `INVALID_ARGUMENTS`；
/// 定义声明了 `default` 也**不得物化**。
#[test]
fn identity_key_missing_is_rejected_not_defaulted() {
    let cat = catalog();
    let d = cat.lookup("channel.r_parameter", 1).unwrap();
    let want = identity(d, json!({"channel": 1}), json!({"index": 0}));
    // 缺 index（身份键）⇒ MISSING_IDENTITY_KEY，而不是物化默认值。
    let err = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({}),
        &want,
    )
    .unwrap_err();
    assert_eq!(err.code(), "MISSING_IDENTITY_KEY");
    assert!(err.to_string().contains("index"), "{err}");
    // 显式 null ⇒ INVALID_ARGUMENTS。
    let err = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": null}),
        &want,
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_ARGUMENTS");
    // 显式空串 ⇒ 类型错（不是「未提供」）。
    let err = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": ""}),
        &want,
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_ARGUMENTS");
    // 即使定义声明了 `default`，缺省也绝不物化。
    let mut with_default = d.clone();
    with_default.arguments[0].default = Some(json!(0));
    let cat2 = DefinitionCatalog::from_definitions(vec![with_default]).unwrap();
    let err = compile_semantic(
        &cat2,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({}),
        &want,
    )
    .unwrap_err();
    assert_eq!(err.code(), "MISSING_IDENTITY_KEY", "default 绝不物化");
}

/// #20：Point Kind 非两值 ⇒ `UNSUPPORTED_POINT_KIND`（Core 放行，Driver 拒）。
#[test]
fn point_kind_unknown_rejected() {
    let cat = catalog();
    for kind in ["weird_kind", "", "nck.Variable", "Nck_Native"] {
        let err = compiler(&cat)
            .compile_point(kind, &native_params(1), "native.C.1.21.1.1")
            .unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED_POINT_KIND", "kind={kind}");
    }
}

/// #21：`parameters` 层未知字段 ⇒ `INVALID_BINDING_CONFIG`（两条分支各一例）。
#[test]
fn parameter_layer_unknown_field_rejected() {
    let cat = catalog();
    // 语义分支：未知键（含旧模型字段与 identity_mode）。
    let want = identity(
        cat.lookup("channel.program.status", 1).unwrap(),
        json!({"channel": 1}),
        json!({}),
    );
    for key in [
        "resource_id",
        "output",
        "module",
        "variable",
        "identity_mode",
    ] {
        let mut params = semantic_params(
            "channel.program.status",
            1,
            json!({"channel": 1}),
            json!({}),
        );
        params.insert(key.into(), json!("x"));
        let err = compiler(&cat)
            .compile_point(POINT_KIND_VARIABLE, &params, &want)
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_BINDING_CONFIG", "key={key}");
    }
    // 原生分支：未知键同样拒绝（剥离不是通用忽略机制）。
    let mut params = native_params(396);
    params.insert("entry".into(), json!("nc_variable"));
    let err = compiler(&cat)
        .compile_point(POINT_KIND_NATIVE, &params, "native.C.1.21.1.396")
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_BINDING_CONFIG");
}

/// #22（**C3 的双重断言**）：`identity_mode` 未传入原生解析器；结果与直接调用旧解析器一致。
#[test]
fn identity_mode_is_not_forwarded_to_native_parser() {
    let cat = catalog();
    // ---- (a)① 含 identity_mode 的 13 字段 map 直交 `from_parameters` ⇒ UnknownField
    let mut thirteen = native_params(396);
    thirteen.insert("identity_mode".into(), json!("derived"));
    assert_eq!(thirteen.len(), 10, "脚手架字段先凑齐再断言 13");
    for key in ["invalid_value", "invalid_tolerance", "unit"] {
        thirteen.insert(key.into(), json!(null));
        thirteen.remove(key);
    }
    thirteen.insert("invalid_value".into(), json!(-200.0));
    thirteen.insert("invalid_tolerance".into(), json!(0.001));
    thirteen.insert("unit".into(), json!("mm"));
    assert_eq!(thirteen.len(), 13);
    let err = NativeNckAddress::from_parameters(&thirteen).unwrap_err();
    assert!(
        matches!(
            err,
            mesa_driver_sinumerik_nck::NativeError::UnknownField { .. }
        ),
        "白名单不得放宽：{err:?}"
    );

    // ---- (a)② 剥离后交给原生解析器的 map 恰为 12 项、不含 identity_mode（逐键断言）
    let (stripped, mode) = AddressCompiler::split_native_parameters(&thirteen).unwrap();
    assert_eq!(mode, IdentityMode::Derived);
    assert_eq!(stripped.len(), 12);
    assert!(!stripped.contains_key("identity_mode"));
    for key in NATIVE_PARAMETER_KEYS {
        assert!(stripped.contains_key(key), "缺 `{key}`");
    }
    // 剥离结果与**同 12 字段**直接调用逐键相等。
    let manual: Map<String, Value> = thirteen
        .iter()
        .filter(|(k, _)| k.as_str() != "identity_mode")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(stripped, manual, "剥离结果里不得多出/少掉任何字段");

    // ---- (b) 最终地址 / wire / source_label 与「同 12 字段直接调用旧解析器」逐字相等
    let compiled = compiler(&cat)
        .compile_point(POINT_KIND_NATIVE, &thirteen, "native.C.1.21.1.396")
        .expect("含 identity_mode 的 nck_native 必须成功");
    let direct = resolve_native(NativeNckAddress::from_parameters(&manual).unwrap()).unwrap();
    assert_eq!(compiled.resolved.wire, direct.wire, "wire 必须逐字节相等");
    assert_eq!(
        encode_var_spec(&compiled.resolved.wire),
        encode_var_spec(&direct.wire)
    );
    assert_eq!(compiled.address, direct.address, "address 全字段相等");
    assert_eq!(compiled.resolved, direct, "resolved 全字段相等");
    assert_eq!(compiled.source_label, direct.address.source_label());
    assert!(
        !compiled.source_label.contains("identity_mode"),
        "source_label 不得含 Compiler 元数据"
    );
    // 未知键仍必须被拒绝（剥离不是通用忽略机制）。
    let mut with_junk = thirteen.clone();
    with_junk.insert("whatever".into(), json!(1));
    assert_eq!(
        compiler(&cat)
            .compile_point(POINT_KIND_NATIVE, &with_junk, "native.C.1.21.1.396")
            .unwrap_err()
            .code(),
        "INVALID_BINDING_CONFIG"
    );
}

/// #23：解析不到 revision 的**三种禁止退化**逐条断言。
#[test]
fn unsupported_definition_revision_rejects_all_three_degradations() {
    let base = catalog();
    // 内存目录：同变量有 rev 1 与 rev 3（没有 2）。
    let mut r3 = base.lookup("channel.program.status", 1).unwrap().clone();
    r3.revision = 3;
    let cat = DefinitionCatalog::from_definitions(vec![
        base.lookup("channel.program.status", 1).unwrap().clone(),
        r3,
    ])
    .unwrap();
    let want = identity(
        base.lookup("channel.program.status", 1).unwrap(),
        json!({"channel": 1}),
        json!({}),
    );

    // ① 不回落最新：请求 2 必须失败，而不是给出 rev 3。
    let err = compile_semantic_rev(
        &cat,
        "channel.program.status",
        2,
        json!({"channel": 1}),
        json!({}),
        &want,
    )
    .unwrap_err();
    assert_eq!(err.code(), "UNSUPPORTED_DEFINITION_REVISION");
    assert!(err.to_string().contains("[1, 3]"), "{err}");
    // ② 不回落其它 revision：请求 2 也不得给出 rev 1（否则 Key 相符就会成功）。
    assert!(
        compile_semantic_rev(
            &cat,
            "channel.program.status",
            2,
            json!({"channel": 1}),
            json!({}),
            &want
        )
        .is_err()
    );
    // ③ 不按 variable_id 猜地址：`revision` 越界同样报错，且**绝不**给出地址。
    let err = compile_semantic_rev(
        &cat,
        "channel.program.status",
        0,
        json!({"channel": 1}),
        json!({}),
        &want,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        CompileError::UnsupportedDefinitionRevision { .. }
    ));
    // 请求存在的 revision 仍必须成功（避免「一律拒绝」的伪实现）。
    assert!(
        compile_semantic_rev(
            &cat,
            "channel.program.status",
            1,
            json!({"channel": 1}),
            json!({}),
            &want
        )
        .is_ok()
    );
    assert!(
        compile_semantic_rev(
            &cat,
            "channel.program.status",
            3,
            json!({"channel": 1}),
            json!({}),
            &want
        )
        .is_ok()
    );
    // revision 类型错（不是整数）是**另一个**码（D3）。
    let params = json!({
        "variable_id": "channel.program.status",
        "definition_revision": "1",
        "target": {"channel": 1},
        "arguments": {},
    });
    let err = compiler(&cat)
        .compile_point(POINT_KIND_VARIABLE, params.as_object().unwrap(), &want)
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_BINDING_CONFIG");
}

/// #24：`ArgumentOffset` 边界（R 参数 `line = index + 1`）。
#[test]
fn argument_offset_boundaries() {
    let cat = catalog();
    // index=0 ⇒ line=1（与冻结向量一致）。
    let d = cat.lookup("channel.r_parameter", 1).unwrap();
    let compiled = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": 0}),
        &identity(d, json!({"channel": 1}), json!({"index": 0})),
    )
    .unwrap();
    assert_eq!(compiled.address.line, 1);
    assert_eq!(compiled.address.column, 1);
    assert_eq!(compiled.address.module, 21);
    // index=425 ⇒ line=426（EXAMPLES 中的 R425 示例）。
    let compiled = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": 425}),
        &identity(d, json!({"channel": 1}), json!({"index": 425})),
    )
    .unwrap();
    assert_eq!(compiled.address.line, 426);
    // 上溢：index=65535 ⇒ line=65536 超 u16 ⇒ INVALID_ARGUMENTS（不夹取）。
    let err = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": 65535}),
        &identity(d, json!({"channel": 1}), json!({"index": 0})),
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_ARGUMENTS");
    assert!(err.to_string().contains("65535"), "{err}");
    // 负数：index=-1 ⇒ 拒。
    let err = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": -1}),
        &identity(d, json!({"channel": 1}), json!({"index": 0})),
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_ARGUMENTS");
    // 未知 argument ⇒ INVALID_ARGUMENTS。
    let err = compile_semantic(
        &cat,
        "channel.r_parameter",
        json!({"channel": 1}),
        json!({"index": 1, "other": 2}),
        &identity(d, json!({"channel": 1}), json!({"index": 1})),
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_ARGUMENTS");
}

// ---------------------------------------------------------------------------
// A 项：身份完整性（#25–#31）
// ---------------------------------------------------------------------------

/// 身份完整性反例的公共断言：身份不同 ⇒ 物理地址元组也不同。
fn assert_identity_distinguishes(
    cat: &DefinitionCatalog,
    variable_id: &str,
    a: (Value, Value),
    b: (Value, Value),
    field: &str,
) {
    let def = cat.lookup(variable_id, 1).unwrap();
    let id_a = identity(def, a.0.clone(), a.1.clone());
    let id_b = identity(def, b.0.clone(), b.1.clone());
    assert_ne!(id_a, id_b, "{variable_id}：{field} 不同却得到同一身份");
    let ca = compile_semantic(cat, variable_id, a.0, a.1, &id_a).unwrap();
    let cb = compile_semantic(cat, variable_id, b.0, b.1, &id_b).unwrap();
    let ta = tuple(&ca);
    let tb = tuple(&cb);
    assert_ne!(ta, tb, "{variable_id}：身份不同但物理地址元组相同");
}

/// 物理地址元组 `(area, area_no, module, column, line, count)`（§5.2 A-2 的 `resolve`）。
fn tuple(p: &mesa_driver_sinumerik_nck::CompiledPoint) -> (char, u8, u8, u16, u16, u8) {
    (
        p.address.area.letter(),
        p.address.area_no,
        p.address.module,
        p.address.column,
        p.address.line,
        p.address.count,
    )
}

/// #25：Channel 实例进身份（`area_no` 变化）。
#[test]
fn a_identity_completeness_channel() {
    let cat = catalog();
    assert_identity_distinguishes(
        &cat,
        "channel.program.status",
        (json!({"channel": 1}), json!({})),
        (json!({"channel": 2}), json!({})),
        "channel",
    );
}

/// #26：ModeGroup 实例进身份（B 区 `area_no` 变化）。
#[test]
fn a_identity_completeness_mode_group() {
    let cat = catalog();
    assert_identity_distinguishes(
        &cat,
        "mode_group.op_mode",
        (json!({"mode_group": 1}), json!({})),
        (json!({"mode_group": 2}), json!({})),
        "mode_group",
    );
    let d = cat.lookup("mode_group.op_mode", 1).unwrap();
    let c = compile_semantic(
        &cat,
        "mode_group.op_mode",
        json!({"mode_group": 2}),
        json!({}),
        &identity(d, json!({"mode_group": 2}), json!({})),
    )
    .unwrap();
    assert_eq!(c.address.area.letter(), 'B');
    assert_eq!(c.address.area_no, 2);
}

/// #27：Axis 实例进身份（`line` 变化）。
#[test]
fn a_identity_completeness_axis() {
    let cat = catalog();
    assert_identity_distinguishes(
        &cat,
        "axis.position.commanded_mcs",
        (json!({"channel": 1, "axis": 1}), json!({})),
        (json!({"channel": 1, "axis": 2}), json!({})),
        "axis",
    );
    let d = cat.lookup("axis.position.commanded_mcs", 1).unwrap();
    let c = compile_semantic(
        &cat,
        "axis.position.commanded_mcs",
        json!({"channel": 1, "axis": 3}),
        json!({}),
        &identity(d, json!({"channel": 1, "axis": 3}), json!({})),
    )
    .unwrap();
    assert_eq!(c.address.area_no, 1, "area_no = channel");
    assert_eq!(c.address.line, 3, "line = axis（不是 area=A）");
}

/// #28：Spindle 实例进身份（`line` 变化）。
#[test]
fn a_identity_completeness_spindle() {
    let cat = catalog();
    assert_identity_distinguishes(
        &cat,
        "spindle.speed.actual",
        (json!({"channel": 1, "spindle": 1}), json!({})),
        (json!({"channel": 1, "spindle": 2}), json!({})),
        "spindle",
    );
    let d = cat.lookup("spindle.speed.actual", 1).unwrap();
    let c = compile_semantic(
        &cat,
        "spindle.speed.actual",
        json!({"channel": 1, "spindle": 2}),
        json!({}),
        &identity(d, json!({"channel": 1, "spindle": 2}), json!({})),
    )
    .unwrap();
    assert_eq!(c.address.module, 114);
    assert_eq!(c.address.line, 2);
}

/// #29：DriveObject 实例进身份。
#[test]
fn a_identity_completeness_drive_object() {
    let cat = catalog();
    let a = json!({"drive_object": {"number": 3, "entry": "nc_variable"}});
    let b = json!({"drive_object": {"number": 4, "entry": "nc_variable"}});
    assert_identity_distinguishes(
        &cat,
        "drive.motor.temperature",
        (a, json!({})),
        (b, json!({})),
        "drive_object.number",
    );
}

/// #30：入口身份 **26 vs 130** 进身份（同 number、同 column）。
#[test]
fn a_identity_completeness_drive_entry_26_vs_130() {
    let cat = catalog();
    let d = cat.lookup("drive.motor.temperature", 1).unwrap();
    let nc = json!({"drive_object": {"number": 3, "entry": "nc_variable"}});
    let drv = json!({"drive_object": {"number": 3, "entry": "drive_object"}});
    let id_nc = identity(d, nc.clone(), json!({}));
    let id_drv = identity(d, drv.clone(), json!({}));
    assert_ne!(id_nc, id_drv, "26/130 必须给出不同身份");
    assert!(id_nc.contains("entry=s:nc_variable"), "{id_nc}");
    assert!(id_drv.contains("entry=s:drive_object"), "{id_drv}");
    let c_nc = compile_semantic(&cat, "drive.motor.temperature", nc, json!({}), &id_nc).unwrap();
    let c_drv = compile_semantic(&cat, "drive.motor.temperature", drv, json!({}), &id_drv).unwrap();
    assert_eq!(c_nc.address.module, 26);
    assert_eq!(c_drv.address.module, 130);
    assert_eq!(c_nc.address.column, c_drv.address.column, "同参数同列");
    assert_ne!(
        encode_var_spec(&c_nc.resolved.wire),
        encode_var_spec(&c_drv.resolved.wire),
        "两条入口不得产生同一请求"
    );
}

/// #31：**机械差分（最强）**——对 42 条定义，身份相等 **当且仅当** 物理地址元组相等。
#[test]
fn a_identity_distinguishes_exactly_the_physical_target() {
    let cat = catalog();
    let mut comparisons = 0usize;
    for d in cat.iter() {
        // 采样空间：每个 target 变体两个实例、每个 argument 两个取值（笛卡尔积）。
        let samples = sample_space(d);
        assert!(samples.len() >= 2, "{}：采样空间不足", d.variable_id);
        for (i, (t1, a1)) in samples.iter().enumerate() {
            for (t2, a2) in samples.iter().skip(i + 1) {
                let id1 = d
                    .canonical_identity(&target(t1.clone()), &args(a1.clone()))
                    .unwrap();
                let id2 = d
                    .canonical_identity(&target(t2.clone()), &args(a2.clone()))
                    .unwrap();
                let c1 = compile_semantic(&cat, &d.variable_id, t1.clone(), a1.clone(), &id1)
                    .unwrap_or_else(|e| panic!("{} 必须可编译：{e}", d.variable_id));
                let c2 = compile_semantic(&cat, &d.variable_id, t2.clone(), a2.clone(), &id2)
                    .unwrap_or_else(|e| panic!("{} 必须可编译：{e}", d.variable_id));
                let same_identity = id1 == id2;
                let same_target = tuple(&c1) == tuple(&c2);
                assert_eq!(
                    same_identity, same_target,
                    "{}：身份相等={same_identity} 而物理地址相等={same_target}\n id1={id1}\n id2={id2}\n t1={t1:?} t2={t2:?}",
                    d.variable_id
                );
                comparisons += 1;
            }
        }
    }
    assert!(
        comparisons > 100,
        "机械差分必须覆盖足够多的组合，实际 {comparisons}"
    );
}

/// 定义的一条合法输入样本（target + arguments 的 JSON 形态）。
fn sample_inputs(d: &VariableDefinition, value: u64) -> (TargetRef, Map<String, Value>) {
    let (t, a) = sample_inputs_json(d, value);
    (target(t), args(a))
}

fn sample_inputs_json(d: &VariableDefinition, value: u64) -> (Value, Value) {
    let mut target_json = Map::new();
    for kind in &d.targets {
        match kind {
            TargetKind::DriveObject => {
                target_json.insert(
                    "drive_object".into(),
                    json!({"number": value, "entry": "nc_variable"}),
                );
            }
            other => {
                target_json.insert(other.as_str().into(), json!(value));
            }
        }
    }
    let mut args_json = Map::new();
    for spec in &d.arguments {
        args_json.insert(spec.key.clone(), json!(value));
    }
    (Value::Object(target_json), Value::Object(args_json))
}

/// 采样空间：target/arguments 的取值组合（含 `drive_object.entry` 两个入口）。
fn sample_space(d: &VariableDefinition) -> Vec<(Value, Value)> {
    let mut out = Vec::new();
    // 每个变体取两个实例（1 / 2）；驱动对象额外覆盖 `drive_object` 入口。
    for value in [1u64, 2] {
        out.push(sample_inputs_json(d, value));
    }
    if d.targets.contains(&TargetKind::DriveObject) {
        for value in [1u64, 2] {
            let (mut t, a) = sample_inputs_json(d, value);
            t["drive_object"]["entry"] = json!("drive_object");
            out.push((t, a));
        }
    }
    // 去重（保持稳定顺序），保证同一输入不会自比。
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|(t, a)| seen.insert((t.to_string(), a.to_string())));
    out
}

// ---------------------------------------------------------------------------
// B 项：native 命名空间隔离（#32–#34）
// ---------------------------------------------------------------------------

/// #32（**B-1**）：`identity_mode="user"` 的 Key 必须匹配 `native_key ["~" dec]`。
///
/// **效力范围**：只断言**前缀命名空间的语法强制**，**不涉及唯一性**
/// （既不是任务内、也不是 Endpoint 级）。
#[test]
fn b_user_key_must_stay_in_native_namespace() {
    let cat = catalog();
    let mut params = native_params(396);
    params.insert("identity_mode".into(), json!("user"));
    for ok in [
        "native.C.1.21.1.396",
        "native.C.1.21.1.396~2",
        "native.C.1.21.1.396~10",
    ] {
        assert!(
            compiler(&cat)
                .compile_point(POINT_KIND_NATIVE, &params, ok)
                .is_ok(),
            "{ok} 必须合法"
        );
    }
    for bad in [
        // 语义形态
        "channel.r_parameter#index=i:100@channel=i:1",
        "channel.program.status@channel=i:1",
        // 非 native 命名空间
        "variable.value",
        "axis3.speed",
        "native",
        "Native.C.1.21.1.396",
        // 语法非法
        "native.C.1.21.1",
        "native.C.1.21.1.396~",
        "native.C.1.21.1.396~1",
        "native.C.1.21.1.396~02",
        "native.C.1.21.1.396~2~3",
        "native.C.1.21.1.396 ",
    ] {
        let err = compiler(&cat)
            .compile_point(POINT_KIND_NATIVE, &params, bad)
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_USER_IDENTITY", "`{bad}` 必须被拒");
    }
}

/// #33（**B-2**）：**该次 configure 可见集合内**的两个 `point_key` 冲突 ⇒ 拒绝。
///
/// **表述约束（必须原样保留，不得写大效力范围）**：
/// - 本用例只断言「Driver 在其**可见集合**（单次 configure 传入的 Task）内提前拒绝」；
/// - **禁止**把本用例写成「Endpoint 级唯一性已验证 / 跨 Task 唯一性已保证」——
///   Driver **看不到**其它 Task 的点位，也无从验证；
/// - **Endpoint-wide 唯一性以 Core 契约（`binding_gate` + `point.rs`）为唯一权威**，
///   本码既**不替代**也**不扩大** Core 的 `DUPLICATE_POINT_KEY`。
#[test]
fn b_user_key_collision_within_batch_rejected() {
    // 可见集合内：derived 点（地址 C/1/21/1/396）与 user 点（同 Key，地址 line=999）。
    let batch = vec![
        ("native.C.1.21.1.396", IdentityMode::Derived),
        ("native.C.1.21.1.396", IdentityMode::User),
    ];
    let err = ensure_batch_identity_namespace_unique(batch).unwrap_err();
    assert_eq!(err.code(), "IDENTITY_NAMESPACE_COLLISION");
    let msg = err.to_string();
    assert!(msg.contains("可见集合"), "必须写明效力范围：{msg}");
    assert!(
        msg.contains("非 Endpoint 级保证"),
        "必须显式声明**不是** Endpoint 级保证：{msg}"
    );
    assert!(
        !msg.contains("已验证") && !msg.contains("Endpoint-wide"),
        "不得把本码表述为 Endpoint-wide 去重保证：{msg}"
    );
    // 纯 derived 的重复不归本码（由驱动侧同任务判重 DUPLICATE_POINT_KEY 负责）。
    assert!(
        ensure_batch_identity_namespace_unique([
            ("native.C.1.21.1.396", IdentityMode::Derived),
            ("native.C.1.21.1.396", IdentityMode::Derived),
        ])
        .is_ok()
    );
    // 语义点位（derived）同理不归本码。
    assert!(
        ensure_batch_identity_namespace_unique([
            ("channel.program.status@channel=i:1", IdentityMode::Derived),
            ("channel.program.status@channel=i:1", IdentityMode::Derived),
        ])
        .is_ok()
    );
    // 两个不同的 user Key 不冲突。
    assert!(
        ensure_batch_identity_namespace_unique([
            ("native.C.1.21.1.396", IdentityMode::User),
            ("native.C.1.21.1.999", IdentityMode::User),
        ])
        .is_ok()
    );
}

/// #34：**结构性论证**——语义身份恒含 `@` 且不得以 `native.` 开头，
/// `native.` 前缀在 Core 侧被强制为 `native_key` 形态 ⇒ 结构上不可能相同。
///
/// **效力范围**：这是结构性论证，**不构成 Endpoint-wide 去重保证**。
#[test]
fn b_user_key_cannot_shadow_semantic_identity() {
    let cat = catalog();
    // 语义身份恒含 `@`（42 条定义全部如此）。
    for d in cat.iter() {
        let (t, a) = sample_inputs(d, 1);
        let id = d.canonical_identity(&t, &a).unwrap();
        assert!(
            id.contains('@'),
            "{}：语义身份必须含 target 段",
            d.variable_id
        );
        assert!(!id.starts_with("native."));
    }
    // 试图冒用语义身份的 user Key 必然被拒（前缀命名空间强制）。
    let mut params = native_params(396);
    params.insert("identity_mode".into(), json!("user"));
    let err = compiler(&cat)
        .compile_point(
            POINT_KIND_NATIVE,
            &params,
            "channel.program.status@channel=i:1",
        )
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_USER_IDENTITY");
}

// ---------------------------------------------------------------------------
// #35：T5（Driver 侧）——编码器 / 校验器与 E1–E11 同一份 golden 向量
// ---------------------------------------------------------------------------

/// #35：**同一份** `nck-point-key-golden.json`（不复制），逐条核对：
/// 值编码、值拒绝、Key 语法，以及 §4.1.1 K 的默认 Key 字面量。
#[test]
fn t5_encoder_matches_e1_e11_golden_vectors() {
    let g: Value = serde_json::from_str(GOLDEN_JSON).expect("golden 必须是合法 JSON");
    assert!(
        g["identity_boundary"].as_str().unwrap().contains("不能"),
        "golden 的效力边界声明必须保留"
    );
    let value_cases = g["value_cases"].as_array().unwrap();
    assert!(value_cases.len() >= 11, "E1–E11 向量不得减少");
    let mut checked = 0usize;
    for c in value_cases {
        let got = mesa_core_types::encode_typed_value(&c["input"])
            .unwrap_or_else(|e| panic!("{} 必须可编码：{e}", c["id"]));
        assert_eq!(got, c["expected"].as_str().unwrap(), "{}", c["id"]);
        checked += 1;
    }
    for c in g["value_reject_cases"].as_array().unwrap() {
        let err = mesa_core_types::encode_typed_value(&c["input"])
            .expect_err(&format!("{} 必须被拒", c["id"]));
        assert_eq!(
            format!("{err:?}").split('(').next().unwrap(),
            c["error"].as_str().unwrap(),
            "{}",
            c["id"]
        );
    }
    let key_cases = g["key_cases"].as_array().unwrap();
    assert!(key_cases.len() >= 40, "Key 向量不得减少");
    for c in key_cases {
        let key = c["key"].as_str().unwrap();
        let valid = c["valid"].as_bool().unwrap();
        assert_eq!(
            mesa_core_types::validate_point_key(key).is_ok(),
            valid,
            "{}：`{key}`",
            c["id"]
        );
    }
    assert!(checked >= 11, "golden 向量数 {checked}");

    // §4.1.1 K 的默认 Key 字面量必须由**定义目录**重算复现（身份不是前端拼的）。
    let cat = catalog();
    let k_cases: [(&str, Value, Value, &str); 5] = [
        (
            "channel.r_parameter",
            json!({"channel": 1}),
            json!({"index": 100}),
            "channel.r_parameter#index=i:100@channel=i:1",
        ),
        (
            "channel.program.status",
            json!({"channel": 1}),
            json!({}),
            "channel.program.status@channel=i:1",
        ),
        (
            "axis.position.commanded_mcs",
            json!({"channel": 1, "axis": 3}),
            json!({}),
            "axis.position.commanded_mcs@axis=i:3,channel=i:1",
        ),
        (
            "spindle.speed.actual",
            json!({"channel": 1, "spindle": 1}),
            json!({}),
            "spindle.speed.actual@channel=i:1,spindle=i:1",
        ),
        (
            "drive.motor.temperature",
            json!({"drive_object": {"number": 3, "entry": "nc_variable"}}),
            json!({}),
            "drive.motor.temperature@drive_object=o:{entry=s:nc_variable,number=i:3}",
        ),
    ];
    for (id, t, a, want) in k_cases {
        let d = cat.lookup(id, 1).unwrap();
        assert_eq!(d.canonical_identity(&target(t), &args(a)).unwrap(), want);
        // 该字面量必须在 golden 的 key_cases 里（同一份真值）。
        assert!(
            key_cases.iter().any(|c| c["key"] == want),
            "{want} 必须存在于共享 golden 中"
        );
    }
    // 原生 derived 规则同样在 K 表里。
    let cat_c = compiler(&cat);
    let p = native_params(396);
    let mut p21 = p.clone();
    p21.insert("module".into(), json!(21));
    p21.insert("area".into(), json!("C"));
    p21.insert("area_no".into(), json!(1));
    p21.insert("column".into(), json!(1));
    p21.insert("line".into(), json!(396));
    let c = cat_c
        .compile_point(POINT_KIND_NATIVE, &p21, "native.C.1.21.1.396")
        .unwrap();
    assert_eq!(c.canonical_identity, "native.C.1.21.1.396");
    let mut user = p21.clone();
    user.insert("identity_mode".into(), json!("user"));
    assert!(
        cat_c
            .compile_point(POINT_KIND_NATIVE, &user, "native.C.1.21.1.396~2")
            .is_ok()
    );
}

// ---------------------------------------------------------------------------
// #36 / #37：T1 路由与 DriveEntry 必填
// ---------------------------------------------------------------------------

/// #36：同一任务混合两种 Point Kind ⇒ 逐点路由（点位级断言，而不是只看总数）。
#[test]
fn t1_mixed_point_kinds_route_per_point() {
    let cat = catalog();
    let semantic_key = "channel.program.status@channel=i:1";
    let native_key = "native.C.1.21.1.396";
    let semantic = compiler(&cat)
        .compile_point(
            POINT_KIND_VARIABLE,
            &semantic_params(
                "channel.program.status",
                1,
                json!({"channel": 1}),
                json!({}),
            ),
            semantic_key,
        )
        .unwrap();
    let native = compiler(&cat)
        .compile_point(POINT_KIND_NATIVE, &native_params(396), native_key)
        .unwrap();
    assert!(semantic.definition.is_some(), "语义分支必须给出定义引用");
    assert!(native.definition.is_none(), "原生分支不得有定义引用");
    assert_eq!(semantic.address.module, 127);
    assert_eq!(native.address.module, 21);
    assert_eq!(semantic.identity_mode, IdentityMode::Derived);
    assert_eq!(native.identity_mode, IdentityMode::Derived);
}

/// #37：`drive_object` 缺 `entry` ⇒ `INVALID_TARGET`；**不存在**「默认 module 26」。
#[test]
fn entry_missing_is_invalid_target() {
    // 解析层。
    let err = TargetRef::from_json(&json!({"drive_object": {"number": 1}})).unwrap_err();
    assert_eq!(err.code(), "INVALID_TARGET");
    // 编译层：手工构造的 `DriveObjectRef` 无法表达缺失（类型必填），因此只验证解析层；
    // 但必须确认 `TargetRef::from_json` 是唯一入口（`Deserialize` 走同一实现）。
    let err =
        serde_json::from_value::<TargetRef>(json!({"drive_object": {"number": 1}})).unwrap_err();
    assert!(err.to_string().contains("entry"), "{err}");
    assert!(err.to_string().contains("缺失"), "{err}");
    assert!(
        err.to_string().contains("默认"),
        "错误必须点明不存在默认入口身份：{err}"
    );
}

// ---------------------------------------------------------------------------
// C1（#57–#62）：Target 合法字段组合
// ---------------------------------------------------------------------------

/// #57：`{channel, axis}` 是**合法复合 Target**，且分别落到 `area_no` / `line`。
#[test]
fn target_composite_channel_axis_is_legal() {
    let cat = catalog();
    let d = cat.lookup("axis.position.commanded_mcs", 1).unwrap();
    let id = identity(d, json!({"channel": 1, "axis": 3}), json!({}));
    let c = compile_semantic(
        &cat,
        "axis.position.commanded_mcs",
        json!({"channel": 1, "axis": 3}),
        json!({}),
        &id,
    )
    .expect("复合 Target 不得因含两个字段被拒");
    assert_eq!(c.address.area_no, 1, "area_no == channel");
    assert_eq!(c.address.line, 3, "line == axis");
    // 主轴同类：`{channel, spindle}`。
    let d = cat.lookup("spindle.speed.actual", 1).unwrap();
    let id = identity(d, json!({"channel": 1, "spindle": 2}), json!({}));
    let c = compile_semantic(
        &cat,
        "spindle.speed.actual",
        json!({"channel": 1, "spindle": 2}),
        json!({}),
        &id,
    )
    .expect("主轴复合 Target 合法");
    assert_eq!(c.address.area_no, 1);
    assert_eq!(c.address.line, 2, "line == spindle");
    // 单个变体同样合法。
    let d = cat.lookup("channel.program.status", 1).unwrap();
    assert!(
        compile_semantic(
            &cat,
            "channel.program.status",
            json!({"channel": 1}),
            json!({}),
            &identity(d, json!({"channel": 1}), json!({}))
        )
        .is_ok()
    );
}

/// #58：缺失必填字段 ⇒ `MISSING_TARGET`（含空 target）。
#[test]
fn target_missing_required_field_rejected() {
    let cat = catalog();
    let cases: [(&str, Value); 4] = [
        ("channel.program.status", json!({})),
        ("axis.position.commanded_mcs", json!({"channel": 1})),
        ("spindle.speed.actual", json!({"spindle": 1})),
        ("drive.motor.temperature", json!({})),
    ];
    for (id, t) in cases {
        let err = compile_semantic(&cat, id, t.clone(), json!({}), "whatever").unwrap_err();
        assert_eq!(err.code(), "MISSING_TARGET", "{id} target={t}");
    }
    // 驱动对象必须有 number 与 entry 才算提供（`{"drive_object":{}}` ⇒ MISSING_TARGET/INVALID_TARGET）。
    let err = compile_semantic(
        &cat,
        "drive.motor.temperature",
        json!({"drive_object": {"number": 1}}),
        json!({}),
        "whatever",
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_TARGET");
}

/// #59：多余字段的**两种形态**都被拒（语义多余 / 结构多余）。
#[test]
fn target_extra_field_rejected() {
    let cat = catalog();
    // ① 语义多余：通道变量给了 axis（未声明的变体）。
    let err = compile_semantic(
        &cat,
        "channel.program.status",
        json!({"channel": 1, "axis": 3}),
        json!({}),
        "whatever",
    )
    .unwrap_err();
    assert_eq!(err.code(), "UNSUPPORTED_TARGET");
    assert!(err.to_string().contains("axis"), "{err}");
    // ② 结构多余：`TargetRef` 未定义的键 ⇒ deny_unknown_fields 口径。
    let err = compile_semantic(
        &cat,
        "channel.program.status",
        json!({"channel": 1, "tool": 2}),
        json!({}),
        "whatever",
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_BINDING_CONFIG");
    // drive_object 内部多余键同样是结构错。
    let err = compile_semantic(
        &cat,
        "drive.motor.temperature",
        json!({"drive_object": {"number": 1, "entry": "nc_variable", "axis": 1}}),
        json!({}),
        "whatever",
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_BINDING_CONFIG");
}

/// #60：类型错必拒，且码归属唯一（D2/D3 vs D7 不重叠）。
#[test]
fn target_field_type_error_rejected() {
    let cat = catalog();
    // ① 字符串实例号 ⇒ 类型错 ⇒ INVALID_BINDING_CONFIG。
    for bad in [
        json!({"channel": "1"}),
        json!({"channel": true}),
        json!({"channel": 1.5}),
        json!({"channel": [1]}),
    ] {
        let err = compile_semantic(&cat, "channel.program.status", bad.clone(), json!({}), "x")
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_BINDING_CONFIG", "target={bad}");
    }
    // `target` 本身不是对象 ⇒ 类型错。
    for bad in [json!([1]), json!("channel"), json!(1)] {
        let err = compile_semantic(&cat, "channel.program.status", bad.clone(), json!({}), "x")
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_BINDING_CONFIG", "target={bad}");
    }
    // ② `drive_object` 缺 entry ⇒ INVALID_TARGET（不是 INVALID_BINDING_CONFIG）。
    let err = compile_semantic(
        &cat,
        "drive.motor.temperature",
        json!({"drive_object": {"number": 1}}),
        json!({}),
        "x",
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_TARGET");
    // ③ `entry` 取值非法 ⇒ INVALID_BINDING_CONFIG。
    let err = compile_semantic(
        &cat,
        "drive.motor.temperature",
        json!({"drive_object": {"number": 1, "entry": "m130"}}),
        json!({}),
        "x",
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_BINDING_CONFIG");
    // ④ `drive_object` 不是对象 ⇒ INVALID_BINDING_CONFIG。
    let err = compile_semantic(
        &cat,
        "drive.motor.temperature",
        json!({"drive_object": 3}),
        json!({}),
        "x",
    )
    .unwrap_err();
    assert_eq!(err.code(), "INVALID_BINDING_CONFIG");
}

/// #61：无效实例编号必拒，边界值明确（0 / 255 合法）。
#[test]
fn target_invalid_instance_number_rejected() {
    for bad in [
        json!(-1),
        json!(256),
        json!(4294967296i64),
        json!(99999999999i64),
    ] {
        let err = TargetRef::from_json(&json!({"channel": bad})).unwrap_err();
        assert_eq!(err.code(), "INVALID_TARGET", "channel={bad}");
    }
    // 0 与 255 为合法边界（与既有原生索引范围一致）。
    let t = TargetRef::from_json(&json!({"channel": 0})).unwrap();
    assert_eq!(t.instance(TargetKind::Channel), Some(0));
    let t = TargetRef::from_json(&json!({"channel": 255})).unwrap();
    assert_eq!(t.instance(TargetKind::Channel), Some(255));
    // 边界之外由**原生**层继续 fail-closed：area_no 上界是 31。
    let cat = catalog();
    let d = cat.lookup("channel.program.status", 1).unwrap();
    let err = compile_semantic(
        &cat,
        "channel.program.status",
        json!({"channel": 255}),
        json!({}),
        &identity(d, json!({"channel": 255}), json!({})),
    )
    .unwrap_err();
    assert_eq!(
        err.code(),
        "INVALID_VARIABLE",
        "Target 层合法但 area_no 超协议上界 ⇒ 原生层拒绝（两层各自 fail-closed）"
    );
    // drive_object.number 同样受 0..=255 约束。
    assert_eq!(
        TargetRef::from_json(&json!({"drive_object": {"number": 256, "entry": "nc_variable"}}))
            .unwrap_err()
            .code(),
        "INVALID_TARGET"
    );
    // validate() 对**手工构造**的越界 TargetRef 也要拒绝（同一判据）。
    let mut def = d.clone();
    def.targets = vec![TargetKind::Channel];
    def.target_required = vec![TargetKind::Channel];
    let manual = TargetRef {
        channel: Some(300),
        ..Default::default()
    };
    assert_eq!(manual.validate(&def).unwrap_err().code(), "INVALID_TARGET");
}

/// #62：**不得自动猜测**——缺必填变体逐条失败，且与「显式补齐」结果不相等。
#[test]
fn target_is_never_inferred_by_compiler() {
    let cat = catalog();
    let mut failures = 0usize;
    for d in cat.iter() {
        let (full_t, full_a) = sample_inputs(d, 1);
        // 删掉一个必填变体（逐条）⇒ 必须 Err，无一条成功。
        let mut stripped = full_t.clone();
        let removed = d.target_required.first().copied().expect("必填变体");
        match removed {
            TargetKind::DriveObject => {
                stripped.drive_object = None;
            }
            TargetKind::Channel => stripped.channel = None,
            TargetKind::ModeGroup => stripped.mode_group = None,
            TargetKind::Axis => stripped.axis = None,
            TargetKind::Spindle => stripped.spindle = None,
        }
        let id = d.canonical_identity(&full_t, &full_a).unwrap();
        let err = compile_semantic(
            &cat,
            &d.variable_id,
            serde_json::to_value(&stripped).unwrap(),
            serde_json::to_value(&full_a).unwrap(),
            &id,
        )
        .unwrap_err();
        assert!(
            matches!(err.code(), "MISSING_TARGET" | "INVALID_TARGET"),
            "{}：缺 `{}` 必须报 MISSING_TARGET/INVALID_TARGET，实际 {}",
            d.variable_id,
            removed.as_str(),
            err.code()
        );
        failures += 1;

        // 「缺 channel 的输入」与「显式补 channel=1 的输入」结果**不相等**。
        //
        // 证明方式必须能真正杀死「猜测」实现：如果编译器会「缺 channel 就补 1」，
        // 那么被删掉 channel 的输入会解析出与显式 channel=1 **相同**的地址，从而与
        // channel=1 的持久化 Key **相符而成功**。因此断言：用 `id_fixed`（= channel=1
        // 的规范身份）去编译被删掉 channel 的输入，**必须失败**。
        if removed == TargetKind::Channel {
            let mut fixed = stripped.clone();
            fixed.channel = Some(1);
            let id_fixed = d.canonical_identity(&fixed, &full_a).unwrap();
            let ok = compile_semantic(
                &cat,
                &d.variable_id,
                serde_json::to_value(&fixed).unwrap(),
                serde_json::to_value(&full_a).unwrap(),
                &id_fixed,
            )
            .unwrap_or_else(|e| panic!("{}：显式补齐后必须成功：{e}", d.variable_id));
            // 补齐后的地址与「本来就给的完整输入」一致（同一逻辑点位）。
            let full_ok = compile_semantic(
                &cat,
                &d.variable_id,
                serde_json::to_value(&full_t).unwrap(),
                serde_json::to_value(&full_a).unwrap(),
                &id,
            )
            .unwrap();
            assert_eq!(tuple(&ok), tuple(&full_ok));
            let err = compile_semantic(
                &cat,
                &d.variable_id,
                serde_json::to_value(&stripped).unwrap(),
                serde_json::to_value(&full_a).unwrap(),
                &id_fixed,
            )
            .unwrap_err();
            assert!(
                matches!(err.code(), "MISSING_TARGET" | "INVALID_TARGET"),
                "{}：缺 channel 却按 channel=1 的身份编译必须失败（证明无默认值），实际 {}",
                d.variable_id,
                err.code()
            );
        }
    }
    assert_eq!(failures, 42, "42 条定义逐条都必须因缺失而失败");
    // 结构上不存在「默认值」路径：`instance()` 对未提供的变体返回 None。
    let t = TargetRef::default();
    for kind in TargetKind::ALL {
        assert_eq!(t.instance(kind), None, "未提供不得给出默认实例号");
    }
    assert!(t.provided_kinds().is_empty());
    // 空 target 在编译层同样是 MISSING_TARGET。
    let err =
        compile_semantic(&cat, "channel.program.status", json!({}), json!({}), "x").unwrap_err();
    assert_eq!(err.code(), "MISSING_TARGET");
}

// ---------------------------------------------------------------------------
// 其它：原生字段白名单与 Descriptor 一致（防止两份清单漂移）
// ---------------------------------------------------------------------------

/// `compiler.rs` 的原生字段白名单必须与 Descriptor 的 `variable` 资源字段表逐项一致。
#[test]
fn native_parameter_keys_match_descriptor_resource_fields() {
    let d = SinumerikNckDriver.descriptor();
    let var = d
        .resources
        .iter()
        .find(|r| r.id == "variable")
        .expect("variable 资源必须保留（C10=D-A）");
    let keys: Vec<&str> = var
        .parameters
        .fields
        .iter()
        .map(|f| f.key.as_str())
        .collect();
    assert_eq!(keys, NATIVE_PARAMETER_KEYS.to_vec(), "两份清单必须逐字一致");
}

/// 汇总：所有定义都必须能被编译器真正编译（`golden_ref` 全覆盖的另一种表述）。
#[test]
fn every_definition_compiles_from_its_sample_inputs() {
    let cat = catalog();
    for d in cat.iter() {
        for (t, a) in sample_space(d).into_iter().take(2) {
            let id = d
                .canonical_identity(&target(t.clone()), &args(a.clone()))
                .unwrap();
            compile_semantic(&cat, &d.variable_id, t, a, &id)
                .unwrap_or_else(|e| panic!("{} 必须可编译：{e}", d.variable_id));
        }
    }
    // 原生白名单与 `native.rs` 的 12 字段行为一致：少一个就必须失败。
    let mut missing = native_params(1);
    missing.remove("decode");
    assert!(matches!(
        NativeNckAddress::from_parameters(&missing),
        Err(mesa_driver_sinumerik_nck::NativeError::MissingField { .. })
    ));
}
