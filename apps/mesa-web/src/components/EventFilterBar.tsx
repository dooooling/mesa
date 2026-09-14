// M3.5 事件过滤器（V2）：常用 + 高级两层。
// - 常用：Active 三态（即时生效）+ 接收时间范围（datetime-local，change 即生效）；
// - 高级（折叠）：Category/Kind/Code/Condition/Severity 精确匹配，文本型统一
//   400ms debounce 后才 onChange（避免每键一次历史 reload）；
// - 受控组件：value 为唯一真相，内部文本态随 value 变化同步（filter 切换/重置
//   时不残留旧输入）；debounce timer 随组件卸载清理。
import { useEffect, useRef, useState } from "react";
import { Button, Col, Collapse, Input, InputNumber, Row, Select, Space } from "antd";
import type { ActiveFilter, EventFilterForm } from "../events/filters";
import { localInputToNs, nsToLocalInput } from "./EventFilters";

const TEXT_DEBOUNCE_MS = 400;

function useDebouncedText(
  value: string,
  onCommit: (v: string) => void,
): [string, (v: string) => void] {
  const [local, setLocal] = useState(value);
  const timer = useRef<number | null>(null);
  const commitRef = useRef(onCommit);
  commitRef.current = onCommit;

  // 外部 value 变化（切换/重置）时同步本地输入，不残留旧文本。
  useEffect(() => {
    setLocal(value);
  }, [value]);

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, []);

  const change = (v: string) => {
    setLocal(v);
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      commitRef.current(v);
    }, TEXT_DEBOUNCE_MS);
  };
  return [local, change];
}

export function EventFilterBar({
  value,
  onChange,
  onReset,
  showConnection,
  connectionValue,
  connectionOptions,
  onConnectionChange,
}: {
  value: EventFilterForm;
  onChange: (next: EventFilterForm) => void;
  onReset: () => void;
  /** 全局页才显示连接下拉（设备页连接由 Workspace 上下文控制，不在此重复）。 */
  showConnection?: boolean;
  connectionValue?: string;
  connectionOptions?: Array<{ value: string; label: string }>;
  onConnectionChange?: (v: string) => void;
}) {
  const set = (patch: Partial<EventFilterForm>) => onChange({ ...value, ...patch });

  const [category, setCategory] = useDebouncedText(value.category ?? "", (v) =>
    onChange({ ...value, category: v }),
  );
  const [kind, setKind] = useDebouncedText(value.kind ?? "", (v) =>
    onChange({ ...value, kind: v }),
  );
  const [code, setCode] = useDebouncedText(value.code ?? "", (v) =>
    onChange({ ...value, code: v }),
  );
  const [conditionId, setConditionId] = useDebouncedText(value.condition_id ?? "", (v) =>
    onChange({ ...value, condition_id: v }),
  );
  const [severity, setSeverity] = useState<number | undefined>(value.severity_min);
  useEffect(() => {
    setSeverity(value.severity_min);
  }, [value.severity_min]);
  const severityTimer = useRef<number | null>(null);
  useEffect(() => {
    return () => {
      if (severityTimer.current !== null) window.clearTimeout(severityTimer.current);
    };
  }, []);
  const changeSeverity = (v: number | null) => {
    const num = typeof v === "number" ? v : undefined;
    setSeverity(num);
    if (severityTimer.current !== null) window.clearTimeout(severityTimer.current);
    severityTimer.current = window.setTimeout(() => {
      onChange({ ...value, severity_min: num });
    }, TEXT_DEBOUNCE_MS);
  };

  return (
    <div style={{ display: "grid", gap: 8 }}>
      <Row gutter={8} align="middle">
        {showConnection ? (
          <Col span={6}>
            <Select
              value={connectionValue ?? "ALL"}
              onChange={onConnectionChange}
              style={{ width: "100%" }}
              options={connectionOptions ?? [{ value: "ALL", label: "全部连接" }]}
            />
          </Col>
        ) : null}
        <Col span={showConnection ? 6 : 8}>
          <Select
            value={value.active}
            onChange={(v: ActiveFilter) => set({ active: v })}
            style={{ width: "100%" }}
            options={[
              { value: "all", label: "全部（Active 不过滤）" },
              { value: "active", label: "Active" },
              { value: "inactive", label: "Inactive" },
            ]}
          />
        </Col>
        <Col span={showConnection ? 6 : 8}>
          <Input
            type="datetime-local"
            value={nsToLocalInput(value.from_ns)}
            onChange={(e) => set({ from_ns: localInputToNs(e.target.value) })}
            placeholder="接收时间起"
            title="接收时间起（received_at_ns）"
            style={{ width: "100%" }}
          />
        </Col>
        <Col span={showConnection ? 6 : 8}>
          <Input
            type="datetime-local"
            value={nsToLocalInput(value.to_ns)}
            onChange={(e) => set({ to_ns: localInputToNs(e.target.value) })}
            placeholder="接收时间止"
            title="接收时间止（received_at_ns）"
            style={{ width: "100%" }}
          />
        </Col>
      </Row>
      <Collapse
        size="small"
        items={[
          {
            key: "advanced",
            label: "高级筛选（精确匹配，输入防抖 400ms）",
            children: (
              <Row gutter={8}>
                <Col span={6}>
                  <Input allowClear placeholder="Category" value={category} onChange={(e) => setCategory(e.target.value)} />
                </Col>
                <Col span={6}>
                  <Input allowClear placeholder="Kind" value={kind} onChange={(e) => setKind(e.target.value)} />
                </Col>
                <Col span={6}>
                  <Input allowClear placeholder="Code" value={code} onChange={(e) => setCode(e.target.value)} />
                </Col>
                <Col span={6}>
                  <Input allowClear placeholder="Condition ID" value={conditionId} onChange={(e) => setConditionId(e.target.value)} />
                </Col>
                <Col span={6} style={{ marginTop: 8 }}>
                  <InputNumber
                    min={0}
                    max={1000}
                    placeholder="Severity ≥（0..1000）"
                    value={severity}
                    onChange={(v) => changeSeverity(typeof v === "number" ? v : null)}
                    style={{ width: "100%" }}
                  />
                </Col>
                <Col span={6} style={{ marginTop: 8 }}>
                  <Space>
                    <Button onClick={onReset}>重置</Button>
                  </Space>
                </Col>
              </Row>
            ),
          },
        ]}
      />
      <div style={{ color: "#525252", fontSize: 12 }}>
        时间过滤对应 received_at_ns（接收时间），非设备发生时间；后端按 seq DESC 分页，默认 100、最大 500。
      </div>
    </div>
  );
}
