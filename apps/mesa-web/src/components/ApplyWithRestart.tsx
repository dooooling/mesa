// 统一生命周期条（P1-5）：Connection / Acquisition / Event subscription
// 共用同一产品语义——配置变化需要重启 Endpoint 才生效。
// - 已停止：直接应用（单个主按钮，文案由调用方定，如“保存订阅”）；
// - 运行中：停止是本次应用动作的一部分（按钮即“停止并应用”），
//   是否恢复运行由“修改后重新启动”复选框决定（默认勾选）。
// 调用方只负责：running 判定、canApply 合法性、onApply(restart) 执行
// Stop → Apply →（可选）Restart 三段式；按钮文案与 409 等错误归调用方。
import { useState } from "react";
import { Alert, Button, Checkbox } from "antd";

export function ApplyWithRestart({
  running,
  applying,
  canApply,
  applyLabel,
  onApply,
}: {
  running: boolean;
  applying: boolean;
  canApply: boolean;
  applyLabel: string;
  onApply: (restart: boolean) => void;
}) {
  const [restart, setRestart] = useState(true);
  if (!running) {
    return (
      <Button type="primary" onClick={() => onApply(false)} loading={applying} disabled={!canApply}>
        {applyLabel}
      </Button>
    );
  }
  return (
    <div style={{ display: "grid", gap: 8 }}>
      <Alert
        type="warning"
        showIcon
        message="Endpoint 正在运行，应用修改需要重启"
        description="停止是本次应用动作的一部分；可选择应用后是否恢复运行。"
      />
      <Checkbox checked={restart} onChange={(e) => setRestart(e.target.checked)}>
        修改后重新启动
      </Checkbox>
      <div>
        <Button type="primary" onClick={() => onApply(restart)} loading={applying} disabled={!canApply}>
          停止并应用
        </Button>
      </div>
    </div>
  );
}
