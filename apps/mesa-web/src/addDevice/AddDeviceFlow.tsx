// M4.2 AddDeviceFlow：替换旧 Onboarding（路由 /devices/new，唯一入口“+ 添加设备”）。
// - 设备 → 连接 → 数据 → 确认四步，前三步只攒 draft，最后一步才编排提交；
// - 连接步（C2）改用共享面板 ConnectionSetupPanel：驱动清单/描述符失败即禁止前进，
//   参数只由当前 Descriptor 物化，校验/探测结果随参数变化立即失效；
//   原先的 FALLBACK_DRIVERS 常量已删除（不得把内置列表当可用驱动）；
// - 数据步用 ResourcePickerAntd：非 NCK 保持 manual 选择；NCK（PR-D）走语义模式
//   （NckPointFields / NckNativeFields），点位只暂存在本地草稿，提交时交给
//   planEndpointTaskSave 统一准入与装配。向导**不传 endpointId**：Endpoint 尚未
//   创建，Browse 必然 404，语义页本身也不需要它；
//   C3-D：**允许零采集点**（离线保存）——此时提交 `acquisition.tasks=[]`，且
//   「创建后启动」默认 false 并禁用，界面明确提示「仅保存，不启动，不立即采集」；
// - 提交走 bootstrapDevice（create→endpoint→tasks→start，失败反向补偿，
//   补偿失败明示残留）；成功进 Device Workspace。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Checkbox, Form, Input, InputNumber, Space, Steps, Tag, message } from "antd";
import { useNavigate } from "react-router-dom";
import type { DriverDescriptor } from "../types";
import {
  buildEndpointCreatePayload,
  isNckDriver,
  suggestEndpointId,
  type NckPointSelection,
  type ResourceSelection,
} from "../deviceModel";
import type { ConnectionValues } from "../connection/connectionSetupModel";
import { ConnectionSetupPanel } from "../connection/ConnectionSetupPanel";
import { ResourcePickerAntd } from "../components/ResourcePickerAntd";
import { applySelectionAdd } from "../resourceSelectionModel";
import {
  classifyNckError,
  describeNckFailure,
  fetchNckCatalog,
  indexNckCatalogDefinitions,
  isNckRequestError,
  type NckCatalog,
} from "../nckCatalog";
import { checkNckFamily } from "../nckPointModel";
import { bootstrapDevice, isDraftComplete, newOperationKey, type AcquisitionDraft, type BootstrapReport, type ConnectionDraft, type DeviceDraft } from "./bootstrap";

export function AddDeviceFlow() {
  const nav = useNavigate();
  const [step, setStep] = useState(0);
  const [deviceForm] = Form.useForm();
  const [connForm] = Form.useForm();
  const [device, setDevice] = useState<DeviceDraft | null>(null);
  const [connection, setConnection] = useState<ConnectionDraft | null>(null);
  const [acquisition, setAcquisition] = useState<AcquisitionDraft | null>(null);

  // 连接步状态：描述符由面板上报（数据步需要 resources 目录），其余由面板自持
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [connName, setConnName] = useState("");
  const [connId, setConnId] = useState("");

  // 数据步状态
  const [sels, setSels] = useState<ResourceSelection[]>([]);
  // PR-D：NCK 语义/原生点位草稿（**仅本地**，不持久化；提交时统一交给保存计划）。
  // NCK 点位绝不进入 `sels`——两种 Binding 结构不可互换。
  const [nckPoints, setNckPoints] = useState<NckPointSelection[]>([]);
  // I-2 定义目录：driver 作用域接口，Endpoint 尚未创建也能读取。
  const [catalog, setCatalog] = useState<NckCatalog | null>(null);
  const [catalogState, setCatalogState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [catalogError, setCatalogError] = useState("");
  const [intervalMs, setIntervalMs] = useState(1000);
  // C3-D：默认**不启动**。离线保存是常态（设备未开机 / 采集点稍后定），
  // 只有用户明确勾选才发送 start: true。
  const [startAfterCreate, setStartAfterCreate] = useState(false);
  // 零采集点时「创建后启动」无意义（会得到一个空转的运行态），UI 上直接禁用并
  // 保持 false；`bootstrapDevice` 里还有第二道兜底。
  const zeroSelections = sels.length === 0 && nckPoints.length === 0;

  const nck = isNckDriver(connection?.driverId);
  const catalogIndex = useMemo(
    () => (catalog ? indexNckCatalogDefinitions(catalog.definitions) : null),
    [catalog],
  );
  // 全量目录（含 identity）：表单内部会再按 scope 过滤；保存计划需要全量才能
  // 对 identity 点位给出 SCOPE_NOT_COLLECTABLE 而不是 DEFINITION_MISSING。
  const definitions = useMemo(() => catalogIndex?.byVariableId ?? null, [catalogIndex]);
  const definitionList = useMemo(
    () => (definitions ? Object.values(definitions) : null),
    [definitions],
  );
  const catalogFamily = catalog?.family ?? null;
  // family 的权威来源 = 连接步 canonical 草稿（与随后落库的 connection 同源）；
  // 目录顶层 family 只是回显，**不**作为提交依据。
  const endpointFamily = useMemo(() => {
    const raw = connection?.connection.family;
    return typeof raw === "string" ? raw.trim() : "";
  }, [connection]);
  // 高级原生 Schema 的唯一来源：Descriptor 的 `variable` 资源（不硬编码字段）。
  const nativeSchema = useMemo(() => {
    if (!nck || !desc) return null;
    return desc.resources?.find((r) => r.id === "variable")?.parameters ?? null;
  }, [nck, desc]);
  const takenNckPointKeys = useMemo(
    () => nckPoints.map((point) => point.point_key),
    [nckPoints],
  );
  const nckDriverId = connection?.driverId ?? "";

  // -------------------------------------------------------------------------
  // 目录加载（PR-D）：进入数据步且当前驱动是 NCK 时，按 **driver 作用域** 读取
  // 完整目录。该接口不建 Endpoint、不开机床连接，因此可以在向导阶段调用。
  // 刻意**不带 family 过滤**（与端点页一致）：family 交叉校验交给 checkNckFamily。
  // 任何失败（含目录不完整）都置空并进入 error —— 绝不保留旧目录继续提交。
  // -------------------------------------------------------------------------
  useEffect(() => {
    if (step !== 2 || !nck || !nckDriverId) return;
    let cancelled = false;
    setCatalogState("loading");
    setCatalogError("");
    setCatalog(null);
    fetchNckCatalog({ driverId: nckDriverId })
      .then((loaded) => {
        if (cancelled) return;
        setCatalog(loaded);
        setCatalogState("ready");
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setCatalog(null);
        setCatalogState("error");
        setCatalogError(
          isNckRequestError(error)
            ? `${error.code}（HTTP ${error.status}，${describeNckFailure(
                classifyNckError(error.status, error.kind),
              )}）：${error.message}`
            : error instanceof Error
              ? error.message
              : String(error),
        );
      });
    return () => {
      cancelled = true;
    };
  }, [step, nck, nckDriverId]);

  /**
   * 接收语义/原生点位。
   *
   * 这里只做「批内唯一性」这道父层职责（与端点页 addPoint 同口径），
   * **不**重新实现身份/表单校验：Key 只能来自 NckPointFields 的服务端
   * identity-preview 或 NckNativeFields 的已冻结规则；Native 的显式确认与
   * 字段校验也由组件自身把关（按钮不可提交时本回调不会被调用）。
   */
  const addNckPoint = (point: NckPointSelection) => {
    if (takenNckPointKeys.includes(point.point_key)) {
      message.error(
        "该 point_key 已被本批点位占用：请调整 Target/Arguments（不得自动改名/加后缀）",
      );
      return false;
    }
    setNckPoints((prev) => [...prev, point]);
    return true;
  };

  /**
   * 显式移除点位：**只影响当前向导草稿**，不发任何网络写请求。
   *
   * 不做直接编辑（不引入第二套编辑状态机）：删除后重新加入必须重新选择变量、
   * 重新运行表单校验并由服务端 Preview 重新计算身份；加入成功时
   * ResourcePickerAntd 会以 nonce 重挂载语义表单，因此不存在复用旧 Preview 的路径。
   */
  const removeNckPoint = (index: number) => {
    setNckPoints((prev) => prev.filter((_, i) => i !== index));
  };

  // 提交状态
  const [submitting, setSubmitting] = useState(false);
  const [report, setReport] = useState<BootstrapReport | null>(null);
  // RC2 修3：operation key（per-operation 幂等）。进入确认页生成一次，
  // 同一 Flow 内重复提交（重试/双击）复用；reset（再建一个）即新操作新 key。
  const [operationKey, setOperationKey] = useState("");

  const draftComplete = useMemo(
    () => isDraftComplete({ device, connection, acquisition }),
    [device, connection, acquisition],
  );

  const commitDevice = async () => {
    try {
      const v = (await deviceForm.validateFields()) as { id: string; name?: string };
      const id = v.id.trim();
      setDevice({ deviceId: id, deviceName: v.name?.trim() || id });
      setStep(1);
    } catch { /* 校验未通过 */ }
  };

  /**
   * 连接步「下一步」：面板已经完成本地归一化（canonical connection），
   * 这里只补齐名称/ID 并组装 draft，不落库。
   */
  const commitConnection = async (args: {
    driverId: string;
    connection: ConnectionValues;
  }) => {
    // ---------------------------------------------------------------------
    // P0 提交守卫：连接步的「下一步」只可能在面板前置门通过后被回调（面板
    // handleSubmit 首行守卫 + hook 的 submitSnapshot 前置门）。这里再判一次入参，
    // 保证**本函数自身**不会在驱动清单/描述符不可用时把连接写进草稿并推进流程
    // ——草稿一旦被污染，第 ④ 步的 /api/v1/device-bootstrap 就会带着不可信连接落库。
    // ---------------------------------------------------------------------
    if (!args?.driverId) return;
    let values: { name?: string; id?: string };
    try {
      values = (await connForm.validateFields()) as { name?: string; id?: string };
    } catch {
      return;
    }
    const payload = buildEndpointCreatePayload({
      deviceId: device?.deviceId ?? "",
      driverId: args.driverId,
      name: (values.name ?? connName).trim() || "connection",
      connection: args.connection,
      id: (values.id ?? connId).trim() || undefined,
    });
    // ---------------------------------------------------------------------
    // P0：连接草稿变化（Driver 或 family）后，旧 NCK 点位可能属于另一系列。
    // 必须整批清空并重载目录，**不做跨 Driver 自动迁移**；否则旧 family 的
    // 语义 Key 会被当成新连接下的有效身份直接提交。
    // ---------------------------------------------------------------------
    const prevDriverId = connection?.driverId ?? "";
    const prevFamily =
      typeof connection?.connection.family === "string" ? connection.connection.family : "";
    const nextFamily =
      typeof payload.connection.family === "string" ? payload.connection.family : "";
    const changed = !!connection && (prevDriverId !== args.driverId || prevFamily !== nextFamily);
    if (changed && (isNckDriver(prevDriverId) || isNckDriver(args.driverId))) {
      setNckPoints([]);
      setCatalog(null);
      setCatalogState("idle");
      setCatalogError("");
      // 旧 canonical 选择在切换后的驱动上同样不可信；NCK 与 selections 互斥。
      setSels([]);
      message.warning(
        "连接草稿已变更（Driver/family）：已清空之前添加的 NCK 点位与目录缓存，请重新选择点位。",
      );
    }
    setConnection({
      driverId: args.driverId,
      endpointId: payload.id,
      endpointName: payload.name,
      connection: payload.connection,
    });
    setConnId(payload.id);
    setStep(2);
  };

  /**
   * 数据步「下一步」：允许**零采集点**（C3-D 离线保存）。
   *
   * 纪律：跳过采集点**不**绕过连接门禁——描述符仍未就绪（`!desc`）时一律不放行，
   * 因为那意味着连接步的前置门被绕过，此时草稿里的连接不可信。
   */
  const commitAcquisition = () => {
    if (!desc) {
      message.error("驱动描述符未就绪，已阻止进入确认页（连接参数未通过描述符校验）。");
      return;
    }
    if (nck) {
      // 目录失败/不完整 ⇒ fail-closed：不进入确认页，也不拿旧目录凑合。
      if (catalogState !== "ready" || !definitions) {
        message.error("NCK 定义目录未就绪或不完整：已阻止进入确认页（不会使用旧目录缓存继续提交）。");
        return;
      }
      // family 缺失 ⇒ 禁止提交（即使零点位也不放行：连接本身不可信）。
      if (!endpointFamily) {
        message.error("连接草稿缺少 family：无法确认系列，禁止提交 NCK 点位。");
        return;
      }
      // 提交前逐点复核 family（复用 checkNckFamily，不新写规则）：
      // 若连接草稿在加入点位后被改动而清空逻辑失效，这里仍会拒绝。
      for (const point of nckPoints) {
        if (point.kind !== "nck_variable") continue;
        const variableId =
          typeof point.parameters.variable_id === "string" ? point.parameters.variable_id : "";
        const definition = definitions[variableId];
        const family = checkNckFamily({
          endpointFamily,
          definitionFamily: definition?.family,
          catalogFamily,
        });
        if (!family.ok) {
          message.error(`点位 ${point.point_key} 的 family 校验失败：${family.code} ${family.message}`);
          return;
        }
      }
    }
    setAcquisition({
      intervalMs,
      // NCK 点位**绝不**进入 selections（两种结构不可互换）。
      selections: nck ? [] : sels,
      nckPoints: nck ? nckPoints : undefined,
      nckDefinitions: nck ? definitions : undefined,
      // 零采集点强制不启动（与 bootstrapDevice 的第二道兜底同口径）
      startAfterCreate: zeroSelections ? false : startAfterCreate,
    });
    setOperationKey(newOperationKey());
    setStep(3);
  };

  const submit = async () => {
    // ---------------------------------------------------------------------
    // P0 提交守卫：`connection` 只可能由上面已守卫的连接步产出，因此「清单失败 /
    // 无可用驱动 / 描述符失败」时这里必然为 null。即便确认页按钮的 disabled 被
    // 回归移除，也绝不发出 POST /api/v1/device-bootstrap。
    // ---------------------------------------------------------------------
    if (!device || !connection || !acquisition) return;
    setSubmitting(true);
    setReport(null);
    try {
      // M5.5：一次原子提交（后端单事务 + 幂等；前端不再分步编排/补偿）。
      // RC2 修3：复用本 Flow 的 operationKey（重试同 key → 重放，不双建）。
      const r = await bootstrapDevice(
        {
          deviceBootstrap: async (b) => {
            const res = await fetch("/api/v1/device-bootstrap", {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify(b),
            });
            const body = await res.json().catch(() => ({}));
            return { status: res.status, body };
          },
        },
        { device, connection, acquisition },
        { idempotencyKey: operationKey || undefined },
      );
      setReport(r);
      if (r.ok) {
        message.success(r.replayed ? `设备 ${r.deviceId} 已存在（幂等重放，未重复创建）` : `已添加设备 ${r.deviceId}`);
      } else if (r.residual) {
        message.error("创建失败，可能存在残留设备/连接，请到设备列表检查");
      } else {
        message.error(`${r.failedMessage ?? "创建失败"}（已自动回滚）`);
      }
    } finally {
      setSubmitting(false);
    }
  };

  const reset = () => {
    setStep(0);
    setDevice(null);
    setConnection(null);
    setAcquisition(null);
    setSels([]);
    // PR-D：再建一个必须回到干净的 NCK 草稿/目录状态（旧目录与旧点位不得复用）。
    setNckPoints([]);
    setCatalog(null);
    setCatalogState("idle");
    setCatalogError("");
    // C3-D：再建一个设备必须回到默认「不启动」，不得继承上一次的勾选状态
    //（否则第二次创建会在用户没有明确选择的情况下启动采集）。
    setStartAfterCreate(false);
    setReport(null);
    setDesc(null);
    // 新操作：旧 operationKey 作废（删除后重建相同配置必须真正执行，
    // 不能命中旧 replay）。
    setOperationKey("");
    deviceForm.resetFields();
    connForm.resetFields();
  };

  return (
    <div style={{ display: "grid", gap: 16, maxWidth: 760 }}>
      <Card size="small" title="添加设备">
        <Steps
          current={step}
          items={[{ title: "设备" }, { title: "连接" }, { title: "数据" }, { title: "确认" }]}
        />
        <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
          前三步只构建草稿，最后一步才真正创建；任一步失败自动回滚。
        </div>
      </Card>

      {step === 0 && (
        <Card size="small" title="① 设备">
          <Form form={deviceForm} layout="vertical">
            <Form.Item name="id" label="名称 *" rules={[{ required: true, message: "设备 ID 必填" }]}>
              <Input placeholder="CNC-01" style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
            </Form.Item>
            <Form.Item name="name" label="显示名">
              <Input placeholder="默认为 ID" />
            </Form.Item>
          </Form>
          <Space>
            <Button type="primary" onClick={commitDevice}>下一步：连接 →</Button>
            <Button onClick={() => nav("/devices")}>返回列表</Button>
          </Space>
        </Card>
      )}

      {step === 1 && (
        <Card size="small" title="② 连接" extra={<Tag>{device?.deviceId}</Tag>}>
          <ConnectionSetupPanel
            mode="create-draft"
            active={step === 1}
            sessionKey={`add-device-connection:${device?.deviceId ?? ""}`}
            submitLabel="下一步：数据 →"
            onSubmit={commitConnection}
            onDescriptorChange={setDesc}
            header={
              <Form form={connForm} layout="vertical">
                <Form.Item name="name" label="连接名称">
                  <Input placeholder="如 FOCAS / OPC UA" value={connName} onChange={(e) => setConnName(e.target.value)} />
                </Form.Item>
                <Form.Item name="id" label="连接 ID（可空自动生成）">
                  <Input
                    placeholder={suggestEndpointId("connection")}
                    value={connId}
                    onChange={(e) => setConnId(e.target.value)}
                    style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}
                  />
                </Form.Item>
              </Form>
            }
          />
          <Space style={{ marginTop: 16 }}>
            <Button onClick={() => setStep(0)}>上一步</Button>
          </Space>
        </Card>
      )}

      {step === 2 && (
        <Card size="small" title="③ 数据">
          {!desc ? (
            <div style={{ fontSize: 12, color: "#525252" }}>驱动描述缺失，无法选择数据点。请返回上一步确认驱动。</div>
          ) : nck ? (
            <>
              {/* NCK：目录是 driver 作用域能力，Endpoint 尚未创建也能加载。
                  失败/不完整即 fail-closed：不渲染可用语义表单，也不允许提交。 */}
              {catalogState === "loading" ? (
                <Alert
                  type="info"
                  showIcon
                  data-testid="nck-catalog-loading"
                  message="正在加载 NCK 定义目录…"
                  description="目录来自 GET /api/v1/drivers/sinumerik-nck/definitions（driver 作用域，不创建 Endpoint、不连接机床）。"
                />
              ) : null}
              {catalogState === "error" ? (
                <Alert
                  type="error"
                  showIcon
                  data-testid="nck-catalog-error"
                  message="NCK 定义目录加载失败：语义点位与提交已禁用（fail-closed）"
                  description={`${catalogError}（不会使用旧目录缓存继续提交）`}
                />
              ) : null}
              {catalogState === "ready" && catalog ? (
                <div
                  data-testid="nck-catalog-meta"
                  style={{ fontSize: 12, color: "#525252" }}
                >
                  定义目录：{catalog.total} 条 / {catalog.pages} 页 / rev {catalog.revision} ·
                  family 取自连接草稿：{endpointFamily || "缺失（禁止提交）"}
                </div>
              ) : null}
              {!endpointFamily ? (
                <Alert
                  type="error"
                  showIcon
                  data-testid="nck-family-missing"
                  message="连接草稿缺少 family：禁止新增/提交 NCK 点位"
                  description="family 必须以连接步的 canonical 草稿为准（目录顶层 family 只是回显，不是提交依据）。"
                />
              ) : null}
              <ResourcePickerAntd
                resources={(desc.resources ?? []) as never[]}
                existingSelections={[] as never}
                selectionMethods={(desc.resource_selection_methods ?? ["manual"]) as never}
                // 刻意**不传 endpointId**：Endpoint 尚未创建，Browse 必然 404；
                // 语义页自身不需要它。传了反而会让语义叶回填被静默吞掉。
                semantic={{
                  driverId: nckDriverId,
                  // null ⇒ 组件 fail-closed，不退回旧 variable 原生字段路径。
                  definitions: definitionList,
                  endpointFamily,
                  catalogFamily,
                  nativeSchema,
                  takenPointKeys: takenNckPointKeys,
                  onAddPoint: addNckPoint,
                }}
              />
              <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
                浏览需保存后到设备详情页使用；向导内不伪造 Browse 入口。已加入 {nckPoints.length} 个 NCK 点位。
              </div>
              {nckPoints.length > 0 ? (
                <div
                  data-testid="nck-draft-points"
                  style={{ marginTop: 8, display: "grid", gap: 6 }}
                >
                  {nckPoints.map((point, i) => (
                    <div
                      key={`${point.point_key}-${i}`}
                      data-testid="nck-draft-point-row"
                      style={{
                        display: "flex",
                        gap: 8,
                        alignItems: "flex-start",
                        flexWrap: "wrap",
                        fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace",
                        fontSize: 12,
                      }}
                    >
                      <code data-testid="nck-draft-point-key">{point.point_key}</code>
                      <Tag>{point.kind}</Tag>
                      <code
                        data-testid="nck-draft-point-parameters"
                        style={{ fontSize: 11, color: "#525252", flex: 1 }}
                      >
                        {JSON.stringify(point.parameters)}
                      </code>
                      <Button
                        size="small"
                        type="link"
                        danger
                        data-testid="nck-remove-point"
                        onClick={() => removeNckPoint(i)}
                      >
                        移除
                      </Button>
                    </div>
                  ))}
                </div>
              ) : null}
              {zeroSelections ? (
                <Alert
                  type="info"
                  showIcon
                  style={{ marginTop: 8 }}
                  message="未选择数据点：将按「仅保存」创建"
                  description="本次只保存设备与连接（后端收到 acquisition.tasks=[]），不启动、不立即采集。设备离线或采集点尚未确定时可以直接下一步，稍后再到设备详情里补采集点并手动启动。"
                />
              ) : null}
              <div style={{ marginTop: 12, display: "flex", gap: 12, alignItems: "center" }}>
                <span style={{ fontSize: 12 }}>采集周期</span>
                <InputNumber min={100} value={intervalMs} onChange={(v) => setIntervalMs(typeof v === "number" ? v : 1000)} />
                <span style={{ fontSize: 12 }}>ms</span>
                <Checkbox
                  checked={!zeroSelections && startAfterCreate}
                  disabled={zeroSelections}
                  onChange={(e) => setStartAfterCreate(e.target.checked)}
                >
                  创建后启动
                </Checkbox>
                <span style={{ fontSize: 12, color: "#525252" }}>
                  {zeroSelections
                    ? "零采集点：创建后为 STOPPED（仅保存配置）"
                    : startAfterCreate
                      ? "创建成功后立即开始采集"
                      : "仅保存，不启动，不立即采集"}
                </span>
              </div>
            </>
          ) : (
            <>
              <ResourcePickerAntd
                resources={(desc.resources ?? []) as never[]}
                existingSelections={sels as never}
                selectionMethods={(desc.resource_selection_methods ?? ["manual"]) as never}
                onAdd={(s) => {
                  // 与 EndpointAcquisitionPane 同一共享实现（终审 blocker：
                  // 两调用方 reconciliation 行为必须一致；此处无 preserved
                  // task，故 protectedSelections=[]）。
                  const resources = (desc.resources ?? []) as Array<{
                    id: string;
                    parameters: { fields: Array<{ key: string; default?: unknown }> };
                  }>;
                  const schemaOf = (resourceId: string) => {
                    const found = resources.find((r) => r.id === resourceId);
                    return {
                      fields: (found?.parameters.fields ?? []).map((f) => ({
                        key: f.key,
                        default: f.default,
                      })),
                    };
                  };
                  const { next, message: msg } = applySelectionAdd({
                    candidate: s as never,
                    editable: sels as never,
                    protectedSelections: [],
                    schemaOf,
                  });
                  if (msg) {
                    if (msg.startsWith("point_key")) message.error(msg);
                    else message.warning(msg);
                    return false;
                  }
                  setSels(next as ResourceSelection[]);
                  return true;
                }}
              />
              <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
                浏览设备（browse）需连接已存在，M4.2 暂只支持手动添加；已选 {sels.length} 项。
              </div>
              {zeroSelections ? (
                <Alert
                  type="info"
                  showIcon
                  style={{ marginTop: 8 }}
                  message="未选择数据点：将按「仅保存」创建"
                  description="本次只保存设备与连接（后端收到 acquisition.tasks=[]），不启动、不立即采集。设备离线或采集点尚未确定时可以直接下一步，稍后再到设备详情里补采集点并手动启动。"
                />
              ) : null}
              {sels.length > 0 && (
                <div style={{ marginTop: 8, display: "grid", gap: 4 }}>
                  {sels.map((s, i) => (
                    <div key={i} style={{ fontSize: 12, fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                      {s.resource_id}
                      <Button size="small" type="link" danger onClick={() => setSels((cur) => cur.filter((_, j) => j !== i))}>移除</Button>
                    </div>
                  ))}
                </div>
              )}
              <div style={{ marginTop: 12, display: "flex", gap: 12, alignItems: "center" }}>
                <span style={{ fontSize: 12 }}>采集周期</span>
                <InputNumber min={100} value={intervalMs} onChange={(v) => setIntervalMs(typeof v === "number" ? v : 1000)} />
                <span style={{ fontSize: 12 }}>ms</span>
                <Checkbox
                  checked={!zeroSelections && startAfterCreate}
                  disabled={zeroSelections}
                  onChange={(e) => setStartAfterCreate(e.target.checked)}
                >
                  创建后启动
                </Checkbox>
                <span style={{ fontSize: 12, color: "#525252" }}>
                  {zeroSelections
                    ? "零采集点：创建后为 STOPPED（仅保存配置）"
                    : startAfterCreate
                      ? "创建成功后立即开始采集"
                      : "仅保存，不启动，不立即采集"}
                </span>
              </div>
            </>
          )}
          <Space style={{ marginTop: 16 }}>
            <Button onClick={() => setStep(1)}>上一步</Button>
            <Button type="primary" onClick={commitAcquisition}>下一步：确认 →</Button>
          </Space>
        </Card>
      )}

      {step === 3 && (
        <Card size="small" title="④ 确认 · 最终按钮才真正提交">
          <pre style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, background: "#f4f4f4", padding: 12 }}>
{`${device?.deviceId}（${device?.deviceName}）
└─ ${connection?.endpointName} [${connection?.driverId}]
   └─ ${
     (acquisition?.nckPoints?.length ?? 0) > 0
       ? `${acquisition?.nckPoints?.length} 个 NCK 点位 · ${acquisition?.intervalMs}ms`
       : acquisition?.selections.length === 0
         ? "零采集点（仅保存连接）"
         : `${acquisition?.selections.length} 个数据点 · ${acquisition?.intervalMs}ms`
   } · ${acquisition?.startAfterCreate ? "创建后启动" : "仅保存（不启动、不立即采集）"}`}
          </pre>
          {!draftComplete && (
            <Alert type="warning" showIcon message="草稿不完整" description="请返回前面步骤补齐设备与连接（采集点可以为空：零采集点将按「仅保存」创建）。" />
          )}
          {report && !report.ok && (
            <Alert
              type={report.residual ? "error" : "warning"}
              showIcon
              style={{ marginBottom: 12 }}
              message={report.residual ? "创建失败，可能存在残留" : "创建失败，已自动回滚"}
              description={[
                `失败步骤：${report.failedStep}（${report.failedMessage ?? ""}）`,
                report.compensated ? "后端已补偿删除未启动的设备/连接。" : null,
                report.residual ? "请到设备列表检查。" : null,
              ].filter(Boolean).join("；")}
            />
          )}
          <Space>
            <Button onClick={() => setStep(2)}>上一步</Button>
            <Button type="primary" loading={submitting} disabled={!draftComplete} onClick={submit}>
              添加设备
            </Button>
            {report?.ok && (
              <Button type="primary" onClick={() => nav(`/devices/${report.deviceId}/overview`)}>
                进入设备 →
              </Button>
            )}
            <Button onClick={reset}>再建一个</Button>
            <Button onClick={() => nav("/devices")}>返回列表</Button>
          </Space>
        </Card>
      )}
    </div>
  );
}
