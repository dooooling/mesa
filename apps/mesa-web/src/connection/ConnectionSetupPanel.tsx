/**
 * 连接配置面板（C2）——三个入口共用的「参数 + 检测 + 动作」主体。
 *
 * 三种模式：
 * - `create-draft`：新建设备流程的连接步（只攒草稿，主按钮「下一步」）；
 * - `add`：给已有设备新增连接（主按钮「创建」）；
 * - `edit`：编辑既有连接（驱动不可改；保留既有 Stop→Apply→可选 Restart 流程）。
 *
 * 纪律：
 * - 驱动清单失败 → 明确报错 + 重试，并且**禁止提交**（不回退到内置驱动常量）；
 * - 描述符失败 → 明确报错 + 重试，并且禁止提交；
 * - 提交前一律 `normalizeConnection` 现场归一化，TSAP 用 raw 文本解析，
 *   因此未失焦就点保存提交的也是最新输入；非法值阻断，不自动修复；
 * - 高级必填缺失时自动展开高级区并滚动定位到第一个缺失字段；
 * - 面板不自己保存结果：结果由 Hook 按 (driverId, revision) 判定，参数一变即失效。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Select, Space, Tag } from "antd";
import type { DriverDescriptor } from "../types";
import { ApplyWithRestart } from "../components/ApplyWithRestart";
import { missingRequiredAdvanced, type ConnectionValues } from "./connectionSetupModel";
import { CONNECTION_FIELD_ID_PREFIX, ConnectionFields } from "./ConnectionFields";
import { ConnectionTestResult } from "./ConnectionTestResult";
import { useConnectionSetup } from "./useConnectionSetup";

export type ConnectionSetupMode = "create-draft" | "add" | "edit";

export interface ConnectionSetupPanelProps {
  mode: ConnectionSetupMode;
  /** 面板不可见（弹窗关闭 / 非当前步骤）时传 false：不发请求 */
  active?: boolean;
  /** 驱动是否可切换；edit 模式必须为 false（driver_id 创建后不可变） */
  driverSelectable?: boolean;
  initialDriverId?: string | null;
  /** edit：服务端连接快照（引用必须稳定，通常放在父组件 state 里） */
  initialValues?: ConnectionValues | null;
  /** 会话标识：变化即整体复位（重新打开弹窗、切换 Endpoint） */
  sessionKey?: string;
  /** edit：Endpoint 是否运行中（决定是否走 Stop→Apply→Restart） */
  running?: boolean;
  /** edit：应用动作进行中 */
  applying?: boolean;
  submitLabel?: string;
  /**
   * 入口侧附加的禁用条件（如 Endpoint 快照未就绪、提交进行中）：
   * 校验、测试与提交**一律**禁用——快照未知时任何动作都可能基于错误前提。
   */
  actionsDisabled?: boolean;
  /** 入口侧自有的字段（名称/ID 等），渲染在驱动选择之前 */
  header?: React.ReactNode;
  onSubmit: (args: {
    driverId: string;
    connection: ConnectionValues;
    restart: boolean;
  }) => void | Promise<void>;
  /** 描述符加载结果上报（新建流程的数据步需要 resources 目录） */
  onDescriptorChange?: (descriptor: DriverDescriptor | null) => void;
  /** 当前驱动上报（入口用它渲染连接 ID 的建议值） */
  onDriverChange?: (driverId: string | null) => void;
}

export function ConnectionSetupPanel(props: ConnectionSetupPanelProps) {
  const {
    mode,
    active = true,
    driverSelectable = mode !== "edit",
    initialDriverId = null,
    initialValues = null,
    sessionKey,
    running = false,
    applying = false,
    submitLabel,
    actionsDisabled = false,
    header,
    onSubmit,
    onDescriptorChange,
    onDriverChange,
  } = props;

  const setup = useConnectionSetup({
    active,
    driverSelectable,
    sessionKey: sessionKey ?? mode,
    initialDriverId,
    initialValues,
    onDescriptorChange,
  });

  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [showMissingHint, setShowMissingHint] = useState(false);
  // nonce 保证「连续两次同样的缺失」也能重新触发滚动定位
  const [focus, setFocus] = useState<{ key: string | null; nonce: number }>({
    key: null,
    nonce: 0,
  });

  // 换驱动即收起高级区（不同驱动的字段集合不可比），并清掉上一次的缺失提示
  useEffect(() => {
    setAdvancedOpen(false);
    setShowMissingHint(false);
  }, [setup.driverId]);

  // 上报当前驱动：入口用它渲染连接 ID 的建议值。放 ref 里避免内联回调进依赖。
  const onDriverChangeRef = useRef(props.onDriverChange);
  onDriverChangeRef.current = props.onDriverChange;
  useEffect(() => {
    onDriverChangeRef.current?.(setup.driverId);
  }, [setup.driverId]);

  useEffect(() => {
    if (!focus.key) return;
    const el = document.getElementById(`${CONNECTION_FIELD_ID_PREFIX}-${focus.key}`);
    // jsdom 无 scrollIntoView；可选调用避免测试环境抛错
    el?.scrollIntoView?.({ block: "center" });
  }, [focus]);

  const usableDrivers = useMemo(
    () => setup.drivers.filter((d) => d.launchable),
    [setup.drivers],
  );
  const blockedDrivers = useMemo(
    () => setup.drivers.filter((d) => !d.launchable),
    [setup.drivers],
  );

  /** 高级必填缺失的兜底：自动展开 + 定位；返回是否可继续。 */
  const guardAdvanced = (): boolean => {
    const missing = missingRequiredAdvanced(setup.fields, setup.raw);
    if (missing.length === 0) {
      setShowMissingHint(false);
      return true;
    }
    setAdvancedOpen(true);
    setShowMissingHint(true);
    setFocus((prev) => ({ key: missing[0], nonce: prev.nonce + 1 }));
    return false;
  };

  const handleValidate = async () => {
    guardAdvanced();
    await setup.validate();
  };

  const handleProbe = async () => {
    guardAdvanced();
    await setup.probeConnection();
  };

  const handleSubmit = async (restart: boolean) => {
    const advancedOk = guardAdvanced();
    // 上一次服务端校验已判非法且参数未改动：不允许绕过校验直接提交
    const priorInvalid = setup.validation?.kind === "invalid";
    const local = setup.validateLocal();
    if (!advancedOk || !local.ok || priorInvalid) return;
    if (!setup.driverId) return;
    await onSubmit({
      driverId: setup.driverId,
      connection: local.normalize.connection,
      restart,
    });
  };

  // 驱动清单/描述符任一不可用即禁止**全部**提交路径（校验/测试/下一步/保存）：
  // 清单失败时既没有可选驱动，也没有可信的参数集合，任何请求都只会误导用户。
  const catalogBlocked = driverSelectable && setup.driversStatus !== "ready";
  const noUsableDriver =
    driverSelectable &&
    setup.driversStatus === "ready" &&
    !usableDrivers.some((d) => d.id === setup.driverId);
  const descriptorBlocked = setup.descriptorStatus !== "ready";
  const driverBlocked = catalogBlocked || noUsableDriver || !setup.driverId;
  /** 校验/测试是否可用：驱动与描述符就绪、入口侧就绪，且没有未重新输入的 Secret marker */
  const canTest = !driverBlocked && !descriptorBlocked && !actionsDisabled && setup.canTestConnection;
  // 保存不受 Secret marker 限制：marker 必须原样回传，后端据此保留历史凭据
  //（若在此处禁用保存，就无法编辑含已存凭据连接的其它字段——属既有能力回归）。
  const canSubmit = !driverBlocked && !descriptorBlocked && !actionsDisabled;

  const primaryLabel = submitLabel ?? (mode === "add" ? "创建" : "下一步");

  return (
    <div style={{ display: "grid", gap: 12 }}>
      {header}

      {driverSelectable ? (
        <div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>驱动（创建后不可改）</div>
          {setup.driversStatus === "loading" ? (
            <div style={{ fontSize: 12, color: "#525252" }}>正在加载驱动清单…</div>
          ) : null}
          {setup.driversStatus === "error" ? (
            <Alert
              type="error"
              showIcon
              message="驱动清单加载失败"
              description={`${setup.driversError}。未拿到服务端清单时不会回退到内置驱动列表，也不会允许提交。`}
              action={
                <Button size="small" onClick={setup.reloadDrivers}>
                  重试
                </Button>
              }
            />
          ) : null}
          {setup.driversStatus === "ready" ? (
            <div style={{ display: "grid", gap: 6 }}>
              <Select
                style={{ width: "100%" }}
                placeholder="请选择可启动的驱动"
                value={setup.driverId ?? undefined}
                onChange={setup.selectDriver}
                options={usableDrivers.map((d) => ({ value: d.id, label: d.name }))}
              />
              {usableDrivers.length === 0 ? (
                <Alert
                  type="warning"
                  showIcon
                  message="没有可启动的驱动"
                  description="服务端未返回任何 launchable=true 的驱动（常见原因：驱动二进制缺失）。请检查部署后重试。"
                />
              ) : null}
              {blockedDrivers.length > 0 ? (
                <div style={{ fontSize: 12, color: "#525252" }}>
                  不可启动（不提供选择）：
                  {blockedDrivers
                    .map((d) => `${d.id}${d.reason ? `（${d.reason}）` : ""}`)
                    .join("；")}
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : (
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span style={{ fontSize: 12 }}>驱动</span>
          <Tag>{setup.driverId ?? "—"}</Tag>
          <span style={{ fontSize: 12, color: "#525252" }}>创建后不可改</span>
        </div>
      )}

      {setup.descriptorStatus === "loading" ? (
        <div style={{ fontSize: 12, color: "#525252" }}>正在加载连接参数描述…</div>
      ) : null}
      {setup.descriptorStatus === "error" ? (
        <Alert
          type="error"
          showIcon
          message="驱动描述符加载失败"
          description={`${setup.descriptorError}。描述符缺失时没有合法的参数依据，已禁止提交。`}
          action={
            <Button size="small" onClick={setup.reloadDescriptor}>
              重试
            </Button>
          }
        />
      ) : null}
      {setup.descriptorStatus === "ready" ? (
        <ConnectionFields
          fields={setup.fields}
          value={setup.raw}
          onChange={setup.setRaw}
          onDraftText={setup.setField}
          advancedOpen={advancedOpen}
          onAdvancedOpenChange={setAdvancedOpen}
          showMissingHint={showMissingHint}
        />
      ) : null}

      {setup.secretMarkers.length > 0 ? (
        <Alert
          type="info"
          showIcon
          message="已保存的凭据不会回显，本场景暂不支持校验/测试"
          description={`连接中的凭据字段（${setup.secretMarkers.join("、")}）仍是「已保存」标记，不是明文。把它当密码发出去既连不上、也会把「凭据未回显」误报成设备不可达，因此这里不发请求。请重新输入凭据后再校验/测试；仅保存配置不受影响，凭据会原样保留。`}
        />
      ) : null}

      <ConnectionTestResult
        validation={setup.validation}
        probe={setup.probe}
        validating={setup.busy === "validate"}
        probing={setup.busy === "probe"}
      />

      <div style={{ display: "grid", gap: 8 }}>
        <Space wrap>
          <Button
            disabled={!canTest}
            onClick={() => void handleValidate()}
            loading={setup.busy === "validate"}
          >
            校验参数
          </Button>
          <Button
            disabled={!canTest}
            onClick={() => void handleProbe()}
            loading={setup.busy === "probe"}
          >
            测试连接
          </Button>
          {mode !== "edit" ? (
            <Button type="primary" disabled={!canSubmit} onClick={() => void handleSubmit(false)}>
              {primaryLabel}
            </Button>
          ) : null}
        </Space>
        {mode === "edit" ? (
          <ApplyWithRestart
            running={running}
            applying={applying}
            canApply={canSubmit}
            applyLabel={submitLabel ?? "保存"}
            onApply={(restart) => {
              void handleSubmit(restart);
            }}
          />
        ) : null}
      </div>
    </div>
  );
}
