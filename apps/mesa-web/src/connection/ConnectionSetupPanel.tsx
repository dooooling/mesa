/**
 * 连接配置面板（C2）——三个入口共用的「参数 + 检测 + 动作」主体。
 *
 * 三种模式：
 * - `create-draft`：新建设备流程的连接步（只攒草稿，主按钮「下一步」）；
 * - `add`：给已有设备新增连接（主按钮「创建」）；
 * - `edit`：编辑既有连接（驱动不可改；保留既有 Stop→Apply→可选 Restart 流程）。
 *
 * C3-C：只有 `edit` + 提供 `endpointId` 时，校验/测试才走 Endpoint-bound
 * `connection-check`——那是唯一能用**已保存凭据**测试的通道（服务端在内存中物化
 * marker，明文不进响应与日志）。`create-draft` / `add` 保持 Driver-bound 路径，
 * marker 依旧禁测。
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
import { selectablePresets, type ConnectionPreset } from "./connectionProfiles";
import { CONNECTION_FIELD_ID_PREFIX, ConnectionFields } from "./ConnectionFields";
import { ConnectionTestResult } from "./ConnectionTestResult";
import { useConnectionSetup } from "./useConnectionSetup";

export type ConnectionSetupMode = "create-draft" | "add" | "edit";

/**
 * 草稿里的 `family` 取值——预设适用范围的**唯一**依据。
 *
 * 只认非空字符串：`family` 是显式必填字段（NCK Descriptor 不提供静默默认），
 * 未选择时必须视为「还不知道系列」，此时不得给出任何示例值。
 */
function draftFamily(raw: ConnectionValues): string | null {
  const value = raw.family;
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/**
 * 示例标题：由 `family` 派生（`828d` → 「828D 实测连接示例」）。
 *
 * 为什么派生而不是给 `ConnectionPreset` 加 `name` 字段：预设的作用范围本来就由
 * `family` 定义，标题只是它的界面表达；另立一份名称表会多出一处可能与 `family`
 * 漂移的事实来源。来源与限制说明一律用预设自带的 `provenance` / `caveat`。
 */
function presetTitle(preset: ConnectionPreset): string {
  return `${preset.family.toUpperCase()} 实测连接示例`;
}

/** 单个预填值的展示文本：十六进制与十进制同时给出，免去用户自己换算 TSAP。 */
function presetValueText(label: string, value: number): string {
  const hex = `0x${value.toString(16).toUpperCase().padStart(4, "0")}`;
  return `${label} ${hex} (${value})`;
}

/** 证据等级标记：`site_tested_example` 必须显式标注为单台实测，不得冒充通用值。 */
function presetScopeTag(preset: ConnectionPreset): { color: string; text: string } {
  return preset.scope === "site_tested_example"
    ? { color: "orange", text: "单台设备验证" }
    : { color: "green", text: "已验证范围" };
}

export interface ConnectionSetupPanelProps {
  mode: ConnectionSetupMode;
  /** 面板不可见（弹窗关闭 / 非当前步骤）时传 false：不发请求 */
  active?: boolean;
  /** 驱动是否可切换；edit 模式必须为 false（driver_id 创建后不可变） */
  driverSelectable?: boolean;
  initialDriverId?: string | null;
  /** edit：服务端连接快照（引用必须稳定，通常放在父组件 state 里） */
  initialValues?: ConnectionValues | null;
  /**
   * C3-C：edit 模式绑定的**已存在 Endpoint** id。
   *
   * 传了它，编辑态的校验/测试才改走 Endpoint-bound `connection-check`（可用已保存
   * 凭据），并据此解除「含 marker 即禁测」。非 edit 模式**一律忽略**该值：
   * 新建流程与新增连接当时还没有 Endpoint，且 marker 绝不能进 Driver-bound Probe。
   */
  endpointId?: string | null;
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
    endpointId = null,
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
    // P1：编辑态必须保留 visible_if 隐藏字段的原值（后端整体替换 connection）
    preserveHiddenValues: mode === "edit",
    // C3-C：只有 edit 模式才可能绑定已存在的 Endpoint。用 `mode === "edit"` 兜一道，
    // 使「新增连接/新建设备仍走 Driver-bound」成为面板层的结构性事实，而不是调用方
    // 的自觉——将来有人给 add 流程传了 endpointId 也不会把 marker 送进 Probe。
    endpointId: mode === "edit" ? endpointId : null,
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

  // -------------------------------------------------------------------------
  // 实测示例预设（C2）：只列**可选**预设（含 `site_tested_example`）。
  //
  // 这里的计算是**纯渲染**：不存在任何 effect 把预设值写进草稿，因此驱动切换、
  // 系列切换、描述符加载完成都不会自动套用示例值（单台实测值不得成为通用默认）。
  // 写入草稿只有一条路径——用户点击「应用实测示例」（见 applyPreset）。
  // -------------------------------------------------------------------------
  const presets = useMemo(
    () => selectablePresets(setup.driverId ?? "", draftFamily(setup.raw)),
    [setup.driverId, setup.raw],
  );

  /**
   * 应用实测示例：**唯一**把预设值写入草稿的入口，只由用户点击触发。
   *
   * 走既有草稿机制 `setup.setField`（而不是绕过它直接改内部状态）：revision 代际、
   * `touched` 语义、上一次校验/探测结果的失效全部沿用，因此应用之后
   * 「校验/测试/保存」拿到的就是这份新值，用户随后也能自由改回或改成别的值。
   *
   * NOTE: 切换 `family` 后已应用的值**不会**被自动清除（`connectionProfiles` 的
   * `fieldsToClearOnSwitch` 尚未接入）。当前取舍是「不擅自删除用户可见的值」；
   * 若后续要求「换系列即清空示例值」，应在此处补一次显式确认后再清理。
   */
  const applyPreset = (preset: ConnectionPreset) => {
    for (const [key, value] of Object.entries(preset.values)) {
      setup.setField(key, value);
    }
    // 预设填的都是高级参数：展开高级区，否则用户看不到刚刚写入了什么。
    setAdvancedOpen(true);
  };

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
    // ---------------------------------------------------------------------
    // P0 提交守卫（**必须是第一行**）：驱动清单 / 可用驱动 / 描述符 / 入口侧
    // 任一不可用即拒绝提交。
    //
    // 为什么按钮已经 disabled 了还要在这里再判一次：`disabled` 只是 UI 表达，
    // 一旦被回归移除（或将来新增程序化提交入口、或有人直接调用 onSubmit），
    // 没有这道守卫就会真的发出 POST /endpoints 或 PUT /endpoints/{id}。
    // 判定与按钮读的是**同一个** `setup.blockReason`（纯函数 submitBlockReason），
    // 因此两侧不可能漂移。
    // ---------------------------------------------------------------------
    if (setup.blockReason !== null || actionsDisabled) return;
    const advancedOk = guardAdvanced();
    // 上一次服务端校验已判非法且参数未改动：不允许绕过校验直接提交
    const priorInvalid = setup.validation?.kind === "invalid";
    const local = setup.validateLocal();
    if (!advancedOk || !local.ok || priorInvalid) return;
    if (!setup.driverId) return;
    // 第二道：hook 侧同一判定 + 编辑态隐藏字段补齐；返回 null 即前置门未过
    const submit = setup.submitSnapshot();
    if (!submit) return;
    await onSubmit({
      driverId: setup.driverId,
      connection: submit.connection,
      restart,
    });
  };

  // 驱动清单/描述符任一不可用即禁止**全部**提交路径（校验/测试/下一步/保存）：
  // 清单失败时既没有可选驱动，也没有可信的参数集合，任何请求都只会误导用户。
  // 注意：判定本身在模型层（submitBlockReason），这里只叠加入口侧条件。
  const driverBlocked = setup.blockReason !== null;
  /**
   * 校验/测试是否可用：驱动与描述符就绪、入口侧就绪，且当前检查通道允许本次载荷。
   *
   * 通道差异（C3-C）封装在 `setup.canTestConnection` 里，面板不重复判定：
   * - Driver-bound：任何未重新输入的 Secret marker 都禁测（marker 不是密码）；
   * - Endpoint-bound：规范 marker 合法（服务端物化），只有位置/形状非法的 marker 才禁测。
   */
  const canTest = !driverBlocked && !actionsDisabled && setup.canTestConnection;
  // 保存不受 Secret marker 限制：marker 必须原样回传，后端据此保留历史凭据
  //（若在此处禁用保存，就无法编辑含已存凭据连接的其它字段——属既有能力回归）。
  const canSubmit = !driverBlocked && !actionsDisabled;

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

      {setup.descriptorStatus === "ready" && presets.length > 0 ? (
        // 紧贴高级参数区：预设填的正是 TSAP 这类高级参数。
        <div style={{ display: "grid", gap: 6 }}>
          {presets.map((preset) => {
            const tag = presetScopeTag(preset);
            return (
              <div
                key={preset.id}
                data-preset-id={preset.id}
                style={{
                  border: "1px solid #d9d9d9",
                  borderRadius: 4,
                  padding: 12,
                  display: "grid",
                  gap: 6,
                }}
              >
                <Space size={8} wrap>
                  <span style={{ fontSize: 13, fontWeight: 600 }}>{presetTitle(preset)}</span>
                  <Tag color={tag.color}>{tag.text}</Tag>
                </Space>
                <Space size={16} wrap>
                  {Object.entries(preset.values).map(([key, value]) => (
                    <span
                      key={key}
                      data-preset-value={key}
                      style={{
                        fontSize: 12,
                        fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace",
                      }}
                    >
                      {presetValueText(
                        setup.fields.find((f) => f.key === key)?.label ?? key,
                        value,
                      )}
                    </span>
                  ))}
                </Space>
                {/* 来源与限制必须可见：单台实测值不是所有同系列设备的通用默认 */}
                <div style={{ fontSize: 12, color: "#525252" }}>{preset.provenance}</div>
                <div style={{ fontSize: 12, color: "#d46b08" }}>{preset.caveat}</div>
                <div>
                  <Button size="small" onClick={() => applyPreset(preset)}>
                    应用实测示例
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      ) : null}

      {setup.secretMarkers.length > 0 ? (
        setup.checkTarget === "endpoint" ? (
          // C3-C：编辑态已接通 Endpoint-bound 检查。规范 marker 会被服务端在内存中
          // 物化成该 Endpoint 的明文（不回显、不落库），因此这里只做说明，不再禁测。
          <Alert
            type="info"
            showIcon
            message="本次校验/测试将使用已保存的凭据（服务端在内存中解密，不回显、不写入配置）"
            description={`连接中的凭据字段（${setup.secretMarkers.join("、")}）仍是「已保存」标记，不是明文。检查请求里保留标记，由服务端按该 Endpoint 的已存凭据物化后交给临时驱动进程；明文不会出现在响应或日志里。若要改用新凭据，直接在字段里输入即可（不会自动保存）。`}
          />
        ) : (
          <Alert
            type="info"
            showIcon
            message="已保存的凭据不会回显，本场景暂不支持校验/测试"
            description={`连接中的凭据字段（${setup.secretMarkers.join("、")}）仍是「已保存」标记，不是明文。把它当密码发出去既连不上、也会把「凭据未回显」误报成设备不可达，因此这里不发请求。请重新输入凭据后再校验/测试；仅保存配置不受影响，凭据会原样保留。`}
          />
        )
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
