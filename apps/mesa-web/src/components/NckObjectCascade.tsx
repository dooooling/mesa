// 联动选择的**界面层**：按变量需要的参数逐级显示下拉，展开下拉时才探测设备。
//
// 为什么不再有「探测实际对象」独立面板：面板是"另一套操作流程"——用户先点按钮、
// 再进一棵树、再挑节点，参数区又是另一组输入框，同一个身份坐标被拆成三条路径。
// 现在只剩一条：**选中变量 → 按需出现级联下拉 → 选实际对象 → 参数由驱动载荷自动填入**。
//
// 纪律（每条都对应一条 fail-closed 要求）：
// - **不挂载即探测**：只有下拉被展开（focus/mousedown/click）才发第一个请求，
//   避免"每个变量都拉起驱动进程读真机"；
// - **端点不可用不探测**：`gate.available === false` 时下拉禁用 + 原因可见，零请求；
// - **失败不得退化成空下拉**：整次探测失败、分支不可达、探测成功但无候选，
//   三种情况的下拉占位文案与提示各不相同，并且都写清"下一步做什么"；
// - **不可用候选照常可见**：驱动判为不可选/载荷无取值的对象以 disabled 选项
//   列出并带稳定码，让用户看得到"为什么这个对象不能用"；
// - **参数只来自载荷**：本组件没有也不接受任何文本输入框。
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Tag } from "antd";
import type { ObjectFill, ObjectNeed, ObjectProbeGate } from "../objectProbe";
import type { NckFormDraft, NckFormModel } from "../nckPointModel";
import {
  cascadeGuidance,
  cascadeKeysOf,
  describeMissingParam,
  evaluateCascadeReadiness,
  isKeyResolved,
  optionsForKey,
  scanObjectCandidates,
  type CascadeDefinitionLike,
  type CascadeKey,
  type CascadeScan,
} from "../objectCascade";

export interface NckObjectCascadeProps {
  /** 探测作用的 Endpoint（缺省即不可探测：不发任何请求）。 */
  endpointId: string;
  /** 探测入口可用性（父层按端点/连接状态算好，缺省按不可用处理）。 */
  gate: ObjectProbeGate;
  /** 当前定义（候选归属判定用；变量切换后候选必须重算）。 */
  definition: CascadeDefinitionLike | null;
  /** 表单模型：决定"还需要哪些参数"（只取必填项）。 */
  model: NckFormModel;
  /** 需要的对象类型（`resolveObjectNeeds` 的结果，用于计算未解析键）。 */
  needs: readonly ObjectNeed[];
  /** 当前草稿（已填的键显示为"已选"，不再提供输入框）。 */
  draft: NckFormDraft;
  /** 是否提供「高级原生配置」入口（决定阻断指引的措辞）。 */
  nativeAvailable: boolean;
  /** 选用某个实际对象后的填参结果（**唯一**的参数来源）。 */
  onFill: (fill: ObjectFill) => void;
  /** 重新选择某一级（清空该键，旧身份随之失效）。 */
  onClear: (key: CascadeKey) => void;
}

type ProbeStatus = "idle" | "loading" | "ok" | "failed" | "unavailable";

export function NckObjectCascade({
  endpointId,
  gate,
  definition,
  model,
  needs,
  draft,
  nativeAvailable,
  onFill,
  onClear,
}: NckObjectCascadeProps) {
  const keys = useMemo(() => cascadeKeysOf(model), [model]);
  const [scan, setScan] = useState<CascadeScan | null>(null);
  const [loading, setLoading] = useState(false);
  // 只采纳最后一次探测的结果（快速切换变量时旧响应不得覆盖新候选）。
  const token = useRef(0);
  const definitionKey = `${String(definition?.variable_id ?? "")}@${String(
    definition?.revision ?? "",
  )}`;

  // 变量一换，上一次的探测结论不再适用：候选属于**这一条**定义。
  useEffect(() => {
    token.current += 1;
    setScan(null);
    setLoading(false);
  }, [definitionKey]);

  /**
   * 探测（**每次变量只做一次**）。
   *
   * 为什么缓存：一次探测会遍历驱动的整棵目录（多个 browse 请求）。逐级选择时每一级
   * 都要用同一份候选，若每级展开都重扫，就会把"选 3 级"变成"读 3 遍真机"。
   * 用户显式点「重新探测」时才强制重扫（`force`）。
   */
  const runScan = (force = false) => {
    if (!gate.available) return;
    if (loading) return;
    if (scan && !force) return;
    const seq = ++token.current;
    setLoading(true);
    void scanObjectCandidates({ endpointId, definition, needs })
      .then((result) => {
        if (seq !== token.current) return;
        setScan(result);
      })
      .finally(() => {
        if (seq !== token.current) return;
        setLoading(false);
      });
  };

  const status: ProbeStatus = !gate.available
    ? "unavailable"
    : loading
      ? "loading"
      : scan === null
        ? "idle"
        : scan.branches[0]?.ok === false
          ? "failed"
          : "ok";

  const readiness = evaluateCascadeReadiness({ keys, draft, scan, gate });
  // 整次探测失败（根分支就不可达）：这属于"探测失败"，与"设备上没有对象"必须区分。
  const rootFailure = scan && scan.branches[0]?.ok === false ? scan.branches[0] : null;

  const placeholderFor = (key: CascadeKey): string => {
    if (!gate.available) return "不可探测（端点不可用）";
    if (loading) return "正在探测实际对象…";
    if (!scan) return "展开下拉以探测实际对象";
    if (rootFailure) return `探测失败（${rootFailure.code}）：没有可用候选`;
    const level = optionsForKey(scan, key);
    if (level.selectable.length) return "请选择实际对象";
    if (scan.incomplete) return "探测未完成（部分分支不可达）：没有可用候选";
    return "设备未返回可提供该参数的可用对象";
  };

  return (
    <div
      data-testid="nck-object-cascade"
      data-probe-status={status}
      style={{ display: "grid", gap: 6 }}
    >
      <div style={{ fontSize: 12, color: "#525252" }}>
        实际对象（逐级选择；参数由驱动返回的选择载荷自动填入，不提供手填）
      </div>

      {/* 端点不可用：只说原因，一个请求都不发。 */}
      {!gate.available ? (
        <div
          data-testid="nck-cascade-gate"
          data-code={gate.code}
          style={{ fontSize: 11, color: "#c62828" }}
        >
          {gate.reason}
        </div>
      ) : null}

      {loading ? (
        <span data-testid="nck-cascade-probe-status" data-status="loading" style={{ fontSize: 11, color: "#8c8c8c" }}>
          正在探测实际对象…
        </span>
      ) : null}

      {/* 整次探测失败：可见、带码，绝不退化成"空下拉"。 */}
      {rootFailure ? (
        <Alert
          type="error"
          showIcon
          data-testid="nck-cascade-error"
          data-code={rootFailure.code}
          message="对象探测失败：不会用空下拉冒充结果"
          description={`${rootFailure.code}：${rootFailure.message}`}
        />
      ) : null}

      {keys.map((key) => {
        const resolved = isKeyResolved(key, draft);
        const value =
          key.source === "target"
            ? (draft.target[key.key] ?? "")
            : (draft.arguments[key.key] ?? "");
        const level = scan ? optionsForKey(scan, key) : { selectable: [], blocked: [] };
        return (
          <div
            key={`${key.source}-${key.key}`}
            data-testid={`nck-object-level-${key.key}`}
            data-resolved={resolved ? "true" : "false"}
            style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}
          >
            <span style={{ width: 140, fontSize: 12 }}>
              {key.label}
              <span style={{ color: "#c62828" }}> *</span>
            </span>
            {resolved ? (
              <>
                <Tag
                  color="green"
                  data-testid={`nck-object-selected-${key.key}`}
                  data-value={value}
                  style={{ marginInlineEnd: 0 }}
                >
                  已选：{value}
                </Tag>
                <Button
                  size="small"
                  data-testid={`nck-object-reselect-${key.key}`}
                  onClick={() => onClear(key)}
                >
                  重新选择
                </Button>
              </>
            ) : (
              <>
                <select
                  data-testid={`nck-object-select-${key.key}`}
                  aria-label={`选择${key.label}`}
                  value=""
                  disabled={!gate.available}
                  // 下拉展开时才探测：focus/按下/点击都算"用户要看了"。
                  onFocus={() => runScan()}
                  onMouseDown={() => runScan()}
                  onClick={() => runScan()}
                  onChange={(event) => {
                    const nodeId = event.target.value;
                    const option = level.selectable.find((item) => item.nodeId === nodeId);
                    if (option?.fill) onFill(option.fill);
                  }}
                  style={{ minWidth: 260, padding: "2px 4px" }}
                >
                  <option value="">{placeholderFor(key)}</option>
                  {level.selectable.map((option) => (
                    <option key={option.nodeId} value={option.nodeId}>
                      {option.label}
                    </option>
                  ))}
                  {/* 不可用候选必须可见：带稳定码说明"为什么不能用"。 */}
                  {level.blocked.map((option) => (
                    <option key={option.nodeId} value="" disabled>
                      {option.label} · 不可用（{option.blocked?.code}）：{option.blocked?.message}
                    </option>
                  ))}
                </select>
                {scan && !rootFailure && level.selectable.length === 0 ? (
                  <span
                    data-testid={`nck-object-level-hint-${key.key}`}
                    data-reason={scan.incomplete ? "probe_incomplete" : "no_candidates"}
                    style={{ fontSize: 11, color: "#c62828" }}
                  >
                    {scan.incomplete
                      ? "部分分支不可达：无法确认设备上是否存在该对象"
                      : "设备没有返回能提供该参数的可用对象"}
                  </span>
                ) : null}
              </>
            )}
          </div>
        );
      })}

      {/* 驱动的异常/无法判定对象：不能成为候选，但必须可见（不是"不存在"）。 */}
      {scan && scan.diagnostics.length ? (
        <div data-testid="nck-cascade-diagnostics" style={{ display: "grid", gap: 2 }}>
          {scan.diagnostics.map((item) => (
            <span
              key={item.nodeId}
              data-testid={`nck-cascade-diagnostic-${item.code}`}
              style={{
                fontSize: 11,
                color: item.severity >= 3 ? "#c62828" : "#8c6000",
              }}
            >
              {item.label}（{item.code}）：{item.message}
            </span>
          ))}
        </div>
      ) : null}

      {/* 缺参数：原因 + 两条出路。不得回退成手填表单，也不得填默认值。 */}
      {readiness.unobtainable.length ? (
        <Alert
          type="error"
          showIcon
          data-testid="nck-cascade-missing"
          data-missing={readiness.unobtainable.map((item) => item.key.key).join(",")}
          data-reason={readiness.unobtainable.map((item) => item.reason).join(",")}
          message="所需参数无法从设备探测获得：已阻断添加"
          description={
            <span>
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                {readiness.unobtainable.map((item) => (
                  <li key={item.key.key} style={{ fontSize: 11 }}>
                    {describeMissingParam(item)}
                  </li>
                ))}
              </ul>
              <span data-testid="nck-cascade-guidance" style={{ fontSize: 11 }}>
                {cascadeGuidance({ gate, nativeAvailable })}
              </span>
            </span>
          }
        />
      ) : null}

      {readiness.missing.some((item) => item.reason === "not_probed") ? (
        <span data-testid="nck-cascade-hint" style={{ fontSize: 11, color: "#525252" }}>
          {readiness.missing
            .filter((item) => item.reason === "not_probed")
            .map((item) => `请展开「${item.key.label}」下拉选择实际对象`)
            .join(" / ")}
        </span>
      ) : null}

      {gate.available && scan && !loading ? (
        <div>
          <Button size="small" data-testid="nck-cascade-retry" onClick={() => runScan(true)}>
            重新探测
          </Button>
        </div>
      ) : null}
    </div>
  );
}
