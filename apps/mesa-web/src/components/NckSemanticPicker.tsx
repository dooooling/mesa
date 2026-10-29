// NCK 语义选型器（PR-C 第三批，PR #150 解耦项）。
//
// 为什么单独成文件：这段逻辑（语义表单 + 语义 Browse）是 **NCK 专属**，此前
// 内嵌在名义通用的 `ResourcePickerAntd.tsx` 里，导致通用组件被迫 import
// nckPointModel / NckPointFields / nck_variable 字面量。现在通用 Picker 不认识
// 任何一点，由调用方（向导/端点页）显式选择本组件。
//
// 纪律：
// - 旧 `variable` 原生字段入口在 NCK 上必然被驱动拒绝，因此**整体不渲染**（A22）；
// - 默认页是 NckPointFields，身份以服务端 Preview 为唯一权威（A21）；
// - 目录缺失 ⇒ fail-closed（不退回旧原生字段流程）；
// - Browse 语义叶的可选性与阻断原因由**驱动**在 wire 上给出（IPC 1.8），
//   本组件只注入载荷解析器 `createNckSemanticHandler()`；准入（scope/family）
//   已搬到驱动侧，前端不再推断（A13/A16 的驱动侧落点见 presets.rs）。
import { useState } from "react";
import { Alert, message, Tabs } from "antd";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";
import { NckPointFields, type NckPreviewFn } from "./NckPointFields";
import type { NckPointSelection } from "../deviceModel";
import type { NckDefinitionLike } from "../nckPointModel";
import { createNckSemanticHandler } from "../nckBrowseSemantic";
import { objectProbeGate, type ObjectProbeGate } from "../objectProbe";
import type { SchemaDescriptor } from "../types";

/** 选型方式声明（来自 Descriptor.resource_selection_methods，缺省即 manual）。 */
export type NckPickerSelectionMethod = "manual" | "browse" | "import";

export interface NckSemanticPickerProps {
  /** 驱动声明的选型方式；只有声明 browse 时才出现浏览页。 */
  selectionMethods?: readonly NckPickerSelectionMethod[];
  /** Browse 作用的 endpoint；向导里尚未创建 Endpoint 时**刻意**不传。 */
  endpointId?: string;
  driverId: string;
  /** 语义目录定义；null/undefined ⇒ fail-closed（不退回旧原生字段流程）。 */
  definitions?: readonly NckDefinitionLike[] | null;
  endpointFamily?: string | null;
  catalogFamily?: string | null;
  /** Descriptor variable 资源 Schema；提供时才开放高级原生页。 */
  nativeSchema?: SchemaDescriptor | null;
  /**
   * 探测入口可用性。缺省按"端点是否存在"推导：没有 Endpoint 的调用方
   * （例如创建向导）必然不可探测，联动选择会明确阻断并给出下一步指引。
   */
  probeGate?: ObjectProbeGate;
  takenPointKeys?: string[];
  /** 语义点位加入回调；未提供时语义表单只读提示，不产生可提交点位。 */
  onAddPoint?: (point: NckPointSelection) => boolean;
  /** 测试注入 Preview；生产不传，走服务端 identity-preview。 */
  preview?: NckPreviewFn;
}

/**
 * NCK 语义选型器。
 *
 * 复用 `NckPointFields`（服务端 Preview 唯一权威），旧 manual 入口在语义模式
 * 下**整体不渲染**（不提供明知会失败的保存操作）。
 */
export function NckSemanticPicker({
  selectionMethods,
  endpointId,
  driverId,
  definitions: rawDefinitions,
  endpointFamily,
  catalogFamily,
  nativeSchema,
  probeGate,
  takenPointKeys,
  onAddPoint,
  preview,
}: NckSemanticPickerProps) {
  const methods: readonly NckPickerSelectionMethod[] =
    selectionMethods && selectionMethods.length ? selectionMethods : ["manual"];
  const [mode, setMode] = useState<"semantic" | "browse">("semantic");
  const [preset, setPreset] = useState<NckPointSelection | null>(null);
  const [nonce, setNonce] = useState(0);
  const definitions = rawDefinitions ? [...rawDefinitions] : null;
  // 没有 Endpoint 就没有可探测对象：探测闸门据此关闭（不发任何请求），
  // 联动选择会明确提示"缺参数且无法探测"并给出下一步，而不是退回手填。
  const resolvedProbeGate: ObjectProbeGate =
    probeGate ??
    objectProbeGate({
      endpointId: endpointId ?? "",
      endpointKnown: !!endpointId,
      connectionKnown: !!endpointId,
    });

  const semanticPane = !definitions ? (
    <div data-testid="nck-semantic-picker-no-definitions-body" style={{ fontSize: 12, color: "#525252" }}>
      定义目录不可用：不会渲染语义表单，也不会退回旧 variable 原生字段流程。
    </div>
  ) : onAddPoint ? (
    <NckPointFields
      key={`picker-semantic-${nonce}-${String(preset?.parameters.variable_id ?? "")}`}
      driverId={driverId}
      endpointFamily={endpointFamily}
      catalogFamily={catalogFamily}
      definitions={definitions ?? []}
      takenPointKeys={takenPointKeys ?? []}
      initialPoint={preset}
      nativeSchema={nativeSchema ?? null}
      endpointId={endpointId ?? null}
      probeGate={resolvedProbeGate}
      preview={preview}
      onSubmit={(point) => {
        const accepted = onAddPoint?.(point) ?? false;
        if (accepted) {
          setPreset(null);
          setNonce((n) => n + 1);
        }
      }}
    />
  ) : (
    <div style={{ fontSize: 12, color: "#c62828" }}>
      上层未提供语义点位接收入口：禁止产生可提交点位。
    </div>
  );

  const browsePane = !endpointId ? (
    <div style={{ fontSize: 12, color: "#525252" }}>浏览需要已创建的 Endpoint。</div>
  ) : (
    <ResourceBrowseAntd
      endpointId={endpointId}
      onFill={() => {
        // 语义模式下不允许回填旧 resource 表单：非语义叶不应被当成 NCK 点位。
        message.warning("请选用语义点位（旧 variable 入口在 NCK 上已被禁用）");
      }}
      semanticHandler={createNckSemanticHandler()}
      onSemanticSelect={
        onAddPoint
          ? (sel) => {
              setPreset({
                kind: sel.pointKind,
                // Browse 的 preview 只是展示；这里置空，由 NckPointFields 的
                // 服务端 Preview 重新确认。
                point_key: "",
                parameters: sel.parameters,
              });
              setNonce((n) => n + 1);
              setMode("semantic");
              message.info("已把 Browse 叶带入语义表单：请等待服务端 Preview 确认身份");
            }
          : undefined
      }
    />
  );

  const tabs = [
    { key: "semantic", label: "语义点位", children: semanticPane },
    ...(methods.includes("browse")
      ? [{ key: "browse", label: "浏览（语义）", children: browsePane }]
      : []),
    // 导入入口在语义模式下同样不渲染（产品未实现，§4.1.1）。
  ];

  return (
    <div style={{ display: "grid", gap: 8 }}>
      <Alert
        type="info"
        data-testid="nck-semantic-picker"
        message="NCK 语义点位"
        description="旧 variable 原生字段入口在 NCK 上必然被驱动拒绝，已不再提供；请在此配置语义点位，身份以服务端 Preview 为准。"
      />
      {!definitions ? (
        <Alert
          type="warning"
          data-testid="nck-semantic-picker-no-definitions"
          message="定义目录不可用：语义点位保持禁用（fail-closed）"
          description="没有目录就无法确认 scope/Target/Arguments；不会退回旧原生字段流程。"
        />
      ) : null}
      <Tabs activeKey={mode} onChange={(k) => setMode(k as "semantic" | "browse")} items={tabs} />
    </div>
  );
}
