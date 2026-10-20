// M2 PointDetailDrawer：点击数据行打开，不跳页面。
// - endpoint_id 只出现在“来源”下，不做主信息（Endpoint 是连接的后端映射）；
// - 只有 /points/latest 快照时不画假趋势；历史/趋势后续再加；
// - [采集设置]/[诊断] 进入同 Workspace 的对应 tab，保留 ?connection=。
import { Button, Descriptions, Drawer, Space, Tag } from "antd";
import { useNavigate } from "react-router-dom";
import { formatAge, formatPointValue } from "../deviceModel";
import {
  CODE_FOOTNOTE,
  COMMUNICATION_LOST_NOTE,
  classifyCode,
  codeGroupTitle,
  codeHex,
  codeText,
  deriveFreshness,
  namespaceLabel,
  samplingBadge,
} from "../qualityCode";
import type { DevicePointView } from "../workspace/useDeviceWorkspaceData";
import { DisplayNameEditor, type RenameTarget } from "./DisplayNameEditor";

const MONO = "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace";

function formatNsTime(ns: unknown): string {
  const n = typeof ns === "number" ? ns : Number(ns);
  if (!Number.isFinite(n) || n <= 0) return "—";
  try {
    return new Date(Math.floor(n / 1e6)).toLocaleString();
  } catch {
    return "—";
  }
}

export function PointDetailDrawer(props: {
  deviceId: string;
  deviceName: string;
  point: DevicePointView | null;
  onClose: () => void;
  /** P2 改名成功回调（永远回填 target；父组件判定是否刷新 Drawer）。 */
  onRenamed?: (target: RenameTarget, display_name: string | null) => void;
}) {
  const { deviceId, deviceName, point, onClose, onRenamed } = props;
  const nav = useNavigate();

  // PR-D1：采样状态与原因码全部**即时派生**（不写回任何 hook/store）。
  const codeClass = point
    ? classifyCode(point.driverId ?? "", point.quality_code, point.quality)
    : "NONE";
  const badge = point
    ? samplingBadge(point.value_origin, point.quality, point.quality_code)
    : null;
  const codeExplanation = point ? codeText(point.driverId ?? "", point.quality_code) : null;

  const gotoAcquisition = () => {
    if (!point) return;
    onClose();
    nav(`/devices/${deviceId}/acquisition?connection=${point.endpoint_id}`);
  };
  const gotoDiagnostics = () => {
    if (!point) return;
    onClose();
    nav(`/devices/${deviceId}/diagnostics?connection=${point.endpoint_id}`);
  };

  return (
    <Drawer
      title={point ? point.displayKey : "数据点"}
      open={!!point}
      onClose={onClose}
      width={480}
    >
      {!point ? null : (
        <div style={{ display: "grid", gap: 16 }}>
          {/* P2：第一行展示名（用户命名优先），第二行 point_key 语义身份。 */}
          <div style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, color: "#525252" }}>
            {point.key ?? point.point_key ?? ""}
          </div>

          <section>
            <h4>展示名</h4>
            <DisplayNameEditor
              point={point}
              onRenamed={(t, n) => onRenamed?.(t, n)}
            />
          </section>

          <section>
            <h4>当前值</h4>
            <div style={{ fontSize: 20 }}>{formatPointValue(point.value)}</div>
          </section>

          <section>
            <h4>状态</h4>
            <Space>
              <Tag color={point.derived === "GOOD" ? "green" : point.derived === "BAD" ? "red" : "orange"}>
                {point.derived}
              </Tag>
              <span style={{ fontSize: 12, color: "#525252" }}>{formatAge(point.ageMs)}未更新</span>
            </Space>
          </section>

          {/*
            PR-D1 U1：采样区按 (value_origin, quality) 二维取文案。
            `CURRENT` 只说明"值由本次 attempt 产生"，不说明采集成功——
            `CURRENT + BAD` 是本次尝试返回的失败结果，必须红色（P1-1）。
          */}
          {badge === null ? null : (
            <section>
              <h4>采样</h4>
              <Space wrap>
                <Tag color={badge.color}>{badge.text}</Tag>
                {badge.abnormal ? <Tag color="orange">异常形态</Tag> : null}
              </Space>
              <div style={{ fontSize: 12, color: "#525252", marginTop: 8 }}>
                最后采样时间：
                {deriveFreshness(point.value_origin) === "NO_VALUE"
                  ? "—（无有效采样）"
                  : `${formatNsTime(point.timestamp_ns)}${
                      badge.notCurrent ? "（非本次 attempt 产生）" : ""
                    }`}
              </div>
              <div style={{ fontSize: 12, color: "#525252" }}>
                源时间：
                {point.source_timestamp_ns === undefined
                  ? "—（协议未提供）"
                  : formatNsTime(point.source_timestamp_ns)}
              </div>
            </section>
          )}

          {/*
            PR-D1 U2–U6：原因码区。`quality_code` 缺失即整块不渲染（GOOD 行零额外 DOM）。
            码与命名空间必须绑定显示；未知码只显示原值 + "不解释"；Mesa 标记
            明确标注"不是设备返回码"，断线覆盖提示"不可恢复"。
          */}
          {point.quality_code === undefined ? null : (
            <section>
              <h4>原因码</h4>
              <div style={{ fontSize: 12, color: "#525252" }}>{codeGroupTitle(codeClass)}</div>
              <div
                data-testid="quality-code-body"
                style={{ marginTop: 6, display: "grid", gap: 4 }}
              >
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <span style={{ fontFamily: MONO }}>
                    {codeHex(point.quality_code) ?? point.quality_code}
                  </span>
                  {namespaceLabel(codeClass) === null ? null : (
                    <Tag style={{ marginInlineEnd: 0 }}>{namespaceLabel(codeClass)}</Tag>
                  )}
                </div>
                {codeExplanation === null ? null : (
                  <div style={{ fontSize: 12 }}>{codeExplanation}</div>
                )}
                {codeClass === "DEVICE_S7_ITEM" && codeExplanation === null ? (
                  <div style={{ fontSize: 12, color: "#525252" }}>仓库无该码定义（待确认）</div>
                ) : null}
                {codeClass === "UNKNOWN" ? (
                  <div style={{ fontSize: 12, color: "#525252" }}>不解释</div>
                ) : null}
                {point.quality_code === "COMMUNICATION_LOST" ? (
                  <div style={{ fontSize: 12 }}>{COMMUNICATION_LOST_NOTE}</div>
                ) : null}
              </div>
              <div style={{ fontSize: 12, color: "#525252", marginTop: 6 }}>{CODE_FOOTNOTE}</div>
            </section>
          )}

          <section>
            <h4>数据类型</h4>
            <Tag>{point.type ?? "—"}</Tag>
          </section>

          <section>
            <h4>来源</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="设备">{deviceName}</Descriptions.Item>
              <Descriptions.Item label="连接">{point.endpointName}</Descriptions.Item>
              {/* P1：来源标签优先；缺失诚实显示"未提供"，不拿 point_key 反推。
                  正常返回标签时不标记。 */}
              <Descriptions.Item label="来源">
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {point.sourceText ?? "—"}
                </span>
                {point.source_label ? null : (
                  <span style={{ fontSize: 12, color: "#525252" }}>（Driver 未提供）</span>
                )}
              </Descriptions.Item>
              <Descriptions.Item label="Endpoint">
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {point.endpoint_id}
                </span>
              </Descriptions.Item>
            </Descriptions>
          </section>

          <section>
            <h4>高级信息</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="Point Key">
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {point.key ?? point.point_key ?? point.point_id}
                </span>
              </Descriptions.Item>
              <Descriptions.Item label="Point ID">
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {point.point_id}
                </span>
              </Descriptions.Item>
            </Descriptions>
          </section>

          <section>
            <h4>时间</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="采集时间">{formatNsTime(point.timestamp_ns)}</Descriptions.Item>
              <Descriptions.Item label="距现在">{formatAge(point.ageMs)}</Descriptions.Item>
            </Descriptions>
          </section>

          <Space>
            <Button type="primary" onClick={gotoAcquisition}>采集设置</Button>
            <Button onClick={gotoDiagnostics}>诊断</Button>
          </Space>
        </div>
      )}
    </Drawer>
  );
}
