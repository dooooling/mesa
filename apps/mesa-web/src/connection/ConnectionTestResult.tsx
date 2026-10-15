/**
 * 连接校验/探测结果的**分层展示**（C2，对应 A08/A09/A10/A11/A26）。
 *
 * 纪律：
 * - 结果只由 `connectionSetupModel` 的 `classifyProbeOutcome` / `describeOutcome` /
 *   `outcomeTone` 产出，本文件不做第二套状态分类，也不按单个 `reachable` 布尔值
 *   宣称「连接成功」；
 * - 所有进入 React 文本节点的内容一律经过 `safeText`：结构化错误对象（如
 *   `{error:{code,message}}`）绝不能作为子节点渲染（否则 "Objects are not valid
 *   as a React child" 直接白屏）；
 * - 400/501/503/504 各有独立语义与修复提示，**不得**统一显示成「CNC 不可达」；
 * - `reachable=true` 但身份缺失时只说「协议可达、身份未确认」，不宣称已识别型号。
 */
import { Alert, Tag } from "antd";
import {
  describeIssue,
  describeOutcome,
  outcomeTone,
  safeText,
  type DeviceIdentity,
  type ProbeOutcome,
} from "./connectionSetupModel";
import type { ValidationState } from "./useConnectionSetup";

const TONE_TO_ALERT: Record<
  ReturnType<typeof outcomeTone>,
  "success" | "info" | "warning" | "error"
> = {
  positive: "success",
  neutral: "info",
  warning: "warning",
  error: "error",
};

/** 各类 outcome 的「下一步该做什么」，避免用户只看到结论不知道怎么办。 */
const FIX_HINTS: Record<ProbeOutcome["kind"], string> = {
  reachable_identified: "协议栈已连通。可继续下一步：添加数据点并启动采集。",
  reachable_unidentified:
    "协议栈可达，但设备未返回可用于识别型号的身份信息；这不代表参数错误，也不代表已确认型号。",
  unreachable:
    "参数已通过校验，但目标设备没有应答。请核对 Host/端口/TSAP、设备是否上电，以及本机到设备的网络与防火墙。",
  invalid: "请按上面的字段逐项修正后重试。Mesa 不会替你自动修正参数。",
  not_found: "该驱动 ID 在当前 mesad 上不存在。请刷新驱动清单后重新选择。",
  unsupported:
    "该驱动没有实现探测接口（不是参数错误，也不是设备不可达）。可以继续保存配置，连接是否真正可用需由启动后的采集结果判断。",
  infrastructure:
    "这是 Mesa 侧（驱动进程/探测服务）不可用，**不是** CNC 不可达。请查看 mesad 与驱动进程状态后重试。",
  timeout: "探测在预算内未完成。请确认设备响应速度与 timeout_ms 设置，或稍后重试。",
  network: "浏览器到 Mesa API 的请求失败。请确认 mesad 在运行、页面地址与端口正确。",
  secret_unavailable:
    "本次没有发出任何探测请求：已保存的凭据不会回显，无法作为密码发送。请重新输入凭据后再测试；若只是保存配置，凭据会原样保留。",
};

function warningList(outcome: ProbeOutcome): string[] {
  if (
    outcome.kind === "reachable_identified" ||
    outcome.kind === "reachable_unidentified" ||
    outcome.kind === "unreachable"
  ) {
    return outcome.warnings.map((w) => safeText(w)).filter((s) => s !== "");
  }
  return [];
}

function identityRows(device: DeviceIdentity): Array<{ label: string; value: string }> {
  const rows: Array<{ label: string; value: string }> = [];
  const push = (label: string, raw: unknown) => {
    const text = safeText(raw);
    if (text !== "") rows.push({ label, value: text });
  };
  push("厂商", device.vendor);
  push("系列", device.family);
  push("型号", device.model);
  push("固件", device.firmware);
  return rows;
}

function Bullets({ items }: { items: string[] }) {
  return (
    <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
      {items.map((text, i) => (
        <li key={`${i}-${text}`}>{text}</li>
      ))}
    </ul>
  );
}

/** 参数校验层：本地归一化与服务端 validate-connection 的结论。 */
function ValidationBlock({ validation }: { validation: ValidationState }) {
  if (validation.kind === "ok") {
    return (
      <Alert
        type="success"
        showIcon
        message="参数校验通过（服务端已接受该连接配置）"
        description="校验只证明参数形状与取值范围合法，不代表设备可达；可达性请用「测试连接」。"
      />
    );
  }
  const description = (
    <div>
      {validation.detail ? <div>{safeText(validation.detail)}</div> : null}
      {validation.issues.length > 0 ? (
        <Bullets items={validation.issues.map(describeIssue)} />
      ) : null}
      <div style={{ marginTop: 4 }}>请修正后重试；Mesa 不会自动改写为上一次的合法值。</div>
    </div>
  );
  return (
    <Alert
      type={validation.kind === "invalid" ? "error" : "warning"}
      showIcon
      message={validation.kind === "invalid" ? "参数校验未通过" : "校验请求未能完成"}
      description={description}
    />
  );
}

/** 探测层：HTTP 状态 + 响应体 → 结构化 outcome → 分层文案。 */
function ProbeBlock({ outcome }: { outcome: ProbeOutcome }) {
  const tone = outcomeTone(outcome);
  const warnings = warningList(outcome);
  const description = (
    <div style={{ display: "grid", gap: 4 }}>
      <div>{FIX_HINTS[outcome.kind]}</div>
      {outcome.kind === "reachable_identified" ? (
        <div>
          <div>设备返回的身份信息：</div>
          <div style={{ display: "grid", gap: 2 }}>
            {identityRows(outcome.device).map((row) => (
              <div key={row.label}>
                {row.label}：{row.value}
              </div>
            ))}
          </div>
        </div>
      ) : null}
      {outcome.kind === "reachable_unidentified" ? (
        <div>
          结论只能是「协议可达、身份未确认」；在拿到设备身份前不得据此宣称已识别型号。
        </div>
      ) : null}
      {outcome.kind === "invalid" && outcome.issues.length > 0 ? (
        <div>
          服务端返回的字段问题：
          <Bullets items={outcome.issues.map((issue) => safeText(issue))} />
        </div>
      ) : null}
      {warnings.length > 0 ? (
        <div>
          警告（原文）：
          <Bullets items={warnings} />
        </div>
      ) : null}
    </div>
  );
  return (
    <Alert
      type={TONE_TO_ALERT[tone]}
      showIcon
      message={`协议探测：${describeOutcome(outcome)}`}
      description={description}
    />
  );
}

export function ConnectionTestResult({
  validation,
  probe,
  validating,
  probing,
}: {
  validation: ValidationState | null;
  probe: ProbeOutcome | null;
  validating?: boolean;
  probing?: boolean;
}) {
  if (!validation && !probe && !validating && !probing) return null;
  return (
    <div style={{ display: "grid", gap: 8 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <Tag>检测结果</Tag>
        <span style={{ fontSize: 12, color: "#525252" }}>
          参数或驱动一旦变化，下面的结果立即失效，需要重新校验/测试。
        </span>
      </div>
      {validating ? <div style={{ fontSize: 12, color: "#525252" }}>正在校验参数…</div> : null}
      {probing ? <div style={{ fontSize: 12, color: "#525252" }}>正在探测设备…</div> : null}
      {validation ? <ValidationBlock validation={validation} /> : null}
      {probe ? <ProbeBlock outcome={probe} /> : null}
    </div>
  );
}
