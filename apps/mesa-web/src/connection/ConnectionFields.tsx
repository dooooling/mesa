/**
 * 连接参数渲染外壳（C2）。
 *
 * 职责边界：
 * - **只做外壳**：字段分组（`ui.advanced` / `ui.order` / `ui.group`）、控件选择、
 *   可见性判定全部由既有的 `components/DescriptorFields.tsx` 负责，本文件不复制
 *   任何字段模板，也不把连接生命周期（校验/探测/提交）塞进 DescriptorFields；
 * - **高级必填缺失的定位**：用 `missingRequiredAdvanced` 从 Descriptor 现场算出
 *   缺失项，交给面板触发的提示与高亮，避免「点保存后后端 400、用户不知道差哪一项」；
 * - **TSAP 未失焦**：把 DescriptorFields 的 `onDraftText` 直接转交外层，使草稿在
 *   每一个字符上都是权威值（归一化仍由外层在提交前完成）。
 */
import { useMemo } from "react";
import { Alert } from "antd";
import { DescriptorFields } from "../components/DescriptorFields";
import type { FieldDescriptor, SchemaDescriptor } from "../types";
import {
  groupFields,
  missingRequiredAdvanced,
  type ConnectionValues,
} from "./connectionSetupModel";

/** 字段锚点前缀：面板据此 scrollIntoView 定位缺失的高级必填字段。 */
export const CONNECTION_FIELD_ID_PREFIX = "connection-field";

export function ConnectionFields({
  fields,
  value,
  disabled,
  onChange,
  onDraftText,
  advancedOpen,
  onAdvancedOpenChange,
  showMissingHint,
}: {
  fields: FieldDescriptor[];
  value: ConnectionValues;
  disabled?: boolean;
  onChange: (next: ConnectionValues) => void;
  onDraftText?: (key: string, text: string) => void;
  /** 受控折叠：面板在「高级必填缺失」时需要强制展开 */
  advancedOpen: boolean;
  onAdvancedOpenChange: (open: boolean) => void;
  /** 是否展示缺失提示（仅在用户点过校验/测试/保存之后才为 true，避免开场噪声） */
  showMissingHint?: boolean;
}) {
  const schema: SchemaDescriptor = useMemo(() => ({ fields }), [fields]);
  // 基础/高级的分组与顺序一律来自 Descriptor（ui.advanced / ui.order），
  // 这里只借用同一份分组结果做提示文案，不在组件里硬编码任何字段模板。
  const grouped = useMemo(() => groupFields(fields), [fields]);
  // 缺失项随当前输入实时收敛：用户补齐后提示与高亮自动消失
  const missing = useMemo(() => missingRequiredAdvanced(fields, value), [fields, value]);
  const hint = showMissingHint === true && missing.length > 0;
  const labels = missing.map((key) => fields.find((f) => f.key === key)?.label ?? key);

  return (
    <div style={{ display: "grid", gap: 8 }}>
      {hint ? (
        <Alert
          type="warning"
          showIcon
          message={`高级参数缺少必填项：${labels.join("、")}`}
          description={`已自动展开高级区并定位到第一个缺失字段（高级参数共 ${grouped.advanced.length} 项）；补齐后才能继续校验/保存。`}
        />
      ) : null}
      <DescriptorFields
        schema={schema}
        value={value}
        disabled={disabled}
        onChange={onChange}
        advancedOpen={advancedOpen}
        onAdvancedOpenChange={onAdvancedOpenChange}
        onDraftText={onDraftText}
        idPrefix={CONNECTION_FIELD_ID_PREFIX}
        highlightKeys={hint ? missing : undefined}
      />
    </div>
  );
}
