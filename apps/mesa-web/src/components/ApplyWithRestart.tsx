// 统一生命周期条（P1-5）：Connection / Acquisition / Event subscription
// 共用同一产品语义——配置变化需要重启 Endpoint 才生效。
//
// P0-2 修复的核心：**按钮文案与底部说明必须描述真实动作**。
// - 已停止：保存只写配置（PUT /tasks），绝不自动启动 ⇒ 按钮「保存配置」，
//   旁注「保存后保持停止」；旧文案「保存并启动」与 onApply(false) 不一致，
//   是最典型的“按钮说一套、代码做一套”。
// - 运行中：停止是本次应用动作的一部分 ⇒ 按钮「停止并应用」，
//   是否恢复运行由「应用后重新启动」复选框决定（默认勾选）。
// - 保存中：按钮「处理中…」并禁用，防止重复点击发出第二次 PUT。
// - 完成后：由 `result` 明确告知实际落点（当前停止 / 采集已启动 / 启动失败），
//   不再让用户从 toast 里猜。
//
// 调用方只负责：running 判定、canApply 合法性、onApply(restart) 执行
// Stop → Apply →（可选）Restart 三段式；各步失败语义由 applyEndpointChange 决定。
import { useState } from "react";
import { Alert, Button, Checkbox } from "antd";

/** 上一次应用的真实落点（由调用方从 LifecycleOutcome 映射，不得猜）。 */
export type ApplyResult = "stopped" | "restarted" | "restart-failed";

export function ApplyWithRestart({
  running,
  applying,
  canApply,
  applyLabel,
  stoppedLabel,
  restart,
  onRestartChange,
  result,
  onApply,
}: {
  running: boolean;
  applying: boolean;
  canApply: boolean;
  /** 运行中不使用该文案（按钮恒为「停止并应用」）；仅作为已停止时的兜底。 */
  applyLabel: string;
  /** 已停止时的按钮文案（默认沿用 applyLabel，如「保存」「保存订阅」）。 */
  stoppedLabel?: string;
  /** 受控勾选：父层需要显示“保存后状态”时传入；不传则组件内部维护。 */
  restart?: boolean;
  onRestartChange?: (value: boolean) => void;
  /** 上一次应用结果；null/undefined 表示尚未应用。 */
  result?: ApplyResult | null;
  onApply: (restart: boolean) => void;
}) {
  const [internalRestart, setInternalRestart] = useState(true);
  const checked = restart ?? internalRestart;
  const setChecked = (value: boolean) => {
    setInternalRestart(value);
    onRestartChange?.(value);
  };

  // 保存中优先展示「处理中…」并禁用：文案切换与 disabled 同时生效，
  // 不给出“还能再点一次”的视觉暗示。
  const buttonText = applying ? "处理中…" : running ? "停止并应用" : stoppedLabel ?? applyLabel;
  const button = (
    <Button
      type="primary"
      data-testid="apply-button"
      loading={applying}
      disabled={applying || !canApply}
      onClick={() => onApply(running ? checked : false)}
    >
      {buttonText}
    </Button>
  );

  const resultAlert =
    result === "restarted" ? (
      <Alert
        type="success"
        showIcon
        data-testid="apply-result"
        message="配置已保存，采集已启动"
      />
    ) : result === "stopped" ? (
      <Alert
        type="success"
        showIcon
        data-testid="apply-result"
        message="配置已保存，当前停止"
      />
    ) : result === "restart-failed" ? (
      <Alert
        type="warning"
        showIcon
        data-testid="apply-result"
        message="配置已保存，但采集启动失败"
        description="配置已写入；启动步骤失败，Endpoint 当前未运行。请检查连接后手动启动。"
      />
    ) : null;

  if (!running) {
    return (
      <div style={{ display: "grid", gap: 8 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          {button}
          <span data-testid="apply-stopped-note" style={{ fontSize: 12, color: "#525252" }}>
            保存后保持停止
          </span>
        </div>
        {resultAlert}
      </div>
    );
  }
  return (
    <div style={{ display: "grid", gap: 8 }}>
      <Alert
        type="warning"
        showIcon
        message="Endpoint 正在运行，应用修改需要重启"
        description={
          checked
            ? "本次保存将执行：停止 → 写入配置 → 启动。"
            : "本次保存将执行：停止 → 写入配置，保存后保持停止。"
        }
      />
      <Checkbox
        data-testid="apply-restart-checkbox"
        checked={checked}
        disabled={applying}
        onChange={(e) => setChecked(e.target.checked)}
      >
        应用后重新启动
      </Checkbox>
      <div>{button}</div>
      {resultAlert}
    </div>
  );
}
