// PR-C 第三批：`nck_native` 高级原生配置表单（A24–A27）。
//
// 为什么单独一个组件：语义表单（NckPointFields）的身份由服务端 Preview 决定；
// 原生表单的身份规则不同（derived 固定规则 / user 用户显式指定）。把两者塞进
// 同一套状态机会让“语义 Key 只读、原生 Key 可显式指定”两条规则互相污染。
//
// 本组件只读 Descriptor 的 `variable` 资源 Schema（12 原生字段），不硬编码
// 地址规则；唯一的前端规则是已冻结的 derived Key 与 user Key 语法（A27/A26）。
import { useMemo, useState } from "react";
import { Alert, Button, Input } from "antd";
import type { SchemaDescriptor } from "../types";
import type { NckPointSelection } from "../deviceModel";
import { DescriptorFields } from "./DescriptorFields";
import { materializeSchemaDefaults } from "../resourceSelectionModel";
import {
  buildNativePointSelection,
  nativeAddressParameters,
  nativeDerivedPointKey,
  validateNativeFields,
  validateNativeUserPointKey,
  type NckNativeIdentityMode,
} from "../nckNativeModel";

export function NckNativeFields({
  schema,
  takenPointKeys,
  initialPoint,
  submitLabel,
  onSubmit,
}: {
  schema: SchemaDescriptor;
  takenPointKeys: string[];
  initialPoint?: NckPointSelection | null;
  submitLabel?: string;
  onSubmit: (point: NckPointSelection) => void;
}) {
  const initialValues = useMemo(() => {
    const base = materializeSchemaDefaults(schema);
    const saved = initialPoint?.parameters
      ? nativeAddressParameters(initialPoint.parameters)
      : {};
    return { ...base, ...saved };
    // 只在挂载时回填；父层用 key 控制编辑对象切换。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [values, setValues] = useState<Record<string, unknown>>(initialValues);
  const initialMode: NckNativeIdentityMode =
    initialPoint?.parameters?.identity_mode === "user" ? "user" : "derived";
  const [identityMode, setIdentityMode] = useState<NckNativeIdentityMode>(initialMode);
  const [userKey, setUserKey] = useState(
    initialMode === "user" ? initialPoint?.point_key ?? "" : "",
  );
  // A24：即使重开已有 user 点位，也只展示原 Key，默认**未确认**；本次保存前
  // 必须由用户重新勾选。确认态只对「当前这一次 Key」有效：任何键入都会撤销。
  const [confirmed, setConfirmed] = useState(false);
  const [submitError, setSubmitError] = useState("");

  const issues = validateNativeFields(schema, values);
  const derivedKey = nativeDerivedPointKey(values);
  const userCheck = validateNativeUserPointKey(userKey);
  const key = identityMode === "derived" ? derivedKey : userCheck.ok ? userCheck.key : null;
  const conflict = !!key && takenPointKeys.includes(key);
  const canSubmit =
    issues.length === 0 &&
    !!key &&
    !conflict &&
    (identityMode === "derived" || (confirmed && userCheck.ok));

  const submit = () => {
    setSubmitError("");
    const built = buildNativePointSelection({ values, identityMode, userKey });
    if (!built.ok) {
      setSubmitError(`${built.code}：${built.message}`);
      return;
    }
    if (takenPointKeys.includes(built.point.point_key)) {
      setSubmitError("point_key 已被 Endpoint 内其它点位占用：请改名（不得自动追加后缀）");
      return;
    }
    onSubmit(built.point);
  };

  return (
    <div style={{ display: "grid", gap: 8 }} data-testid="nck-native-fields">
      <Alert
        type="warning"
        data-testid="nck-native-warning"
        message="高级原生配置（nck_native）"
        description="该模式直接提交 12 个原生字段，不经过语义定义目录；语义点位请使用「语义点位」页。derived 身份按固定规则生成，user 身份由你显式指定且不会被自动改名。"
      />

      <div style={{ fontSize: 12, color: "#525252" }}>身份模式（identity_mode）</div>
      <label style={{ display: "flex", gap: 6, alignItems: "center" }}>
        <input
          type="radio"
          name="nck-native-identity-mode"
          value="derived"
          checked={identityMode === "derived"}
          onChange={() => setIdentityMode("derived")}
        />
        derived（默认）：Key 由固定规则生成
      </label>
      <label style={{ display: "flex", gap: 6, alignItems: "center" }}>
        <input
          type="radio"
          name="nck-native-identity-mode"
          value="user"
          checked={identityMode === "user"}
          onChange={() => {
            setIdentityMode("user");
            // 从 derived 切到 user 时，把当前 derived Key 作为**初始文本**，
            // 但身份仍由用户最终确认（不自动改名、不自动追加后缀）。
            if (!userKey && derivedKey) setUserKey(derivedKey);
            setConfirmed(false);
          }}
        />
        user（显式指定 Native Key）
      </label>

      {identityMode === "user" ? (
        <>
          <Input
            data-testid="nck-native-user-key"
            aria-label="native-user-key"
            value={userKey}
            onChange={(e) => {
              setUserKey(e.target.value);
              // A24：Key 一变，上一次确认立即失效（防止「确认后偷改」）。
              setConfirmed(false);
            }}
            placeholder="native.<area>.<area_no>.<module>.<column>.<line>"
            status={!userCheck.ok && userKey !== "" ? "error" : undefined}
          />
          {!userCheck.ok && userKey !== "" ? (
            <span data-testid="nck-native-key-error" style={{ fontSize: 12, color: "#c62828" }}>
              {userCheck.message}
            </span>
          ) : null}
          <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 12 }}>
            <input
              type="checkbox"
              data-testid="nck-native-user-confirm"
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            我确认使用自定义 Native Key（不会自动改名/追加后缀）
          </label>
        </>
      ) : null}

      <div style={{ fontSize: 12, color: "#525252" }}>原生字段（来自 Descriptor variable 资源）</div>
      <DescriptorFields schema={schema} value={values} onChange={setValues} />

      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>point_key</span>
        {key ? (
          <code data-testid="nck-native-key" style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 12 }}>
            {key}
          </code>
        ) : (
          <span data-testid="nck-native-key-empty" style={{ fontSize: 12, color: "#8c8c8c" }}>
            （原生字段不完整或 user Key 非法）
          </span>
        )}
        {identityMode === "derived" ? (
          <span style={{ fontSize: 12, color: "#525252" }}>
            固定规则：native.&lt;area&gt;.&lt;area_no&gt;.&lt;module&gt;.&lt;column&gt;.&lt;line&gt;
          </span>
        ) : null}
      </div>

      {issues.length > 0 ? (
        <Alert
          type="warning"
          data-testid="nck-native-issues"
          message={`原生字段未通过 Descriptor 约束（${issues.length} 项）`}
          description={
            <ul style={{ margin: 0, paddingLeft: 18 }}>
              {issues.map((issue, idx) => (
                <li key={idx} style={{ fontSize: 11 }}>
                  {issue.code} · {issue.message}
                </li>
              ))}
            </ul>
          }
        />
      ) : null}

      {conflict ? (
        <span data-testid="nck-native-conflict" style={{ fontSize: 12, color: "#c62828" }}>
          该 point_key 已被 Endpoint 内其它点位占用：请自行改名（不得自动追加 ~&lt;n&gt; 后缀）
        </span>
      ) : null}
      {submitError ? (
        <span data-testid="nck-native-submit-error" style={{ fontSize: 12, color: "#c62828" }}>
          {submitError}
        </span>
      ) : null}

      <div>
        <Button
          type="primary"
          data-testid="nck-native-add-point"
          disabled={!canSubmit}
          onClick={submit}
        >
          {submitLabel ?? "加入原生点位"}
        </Button>
      </div>
    </div>
  );
}
