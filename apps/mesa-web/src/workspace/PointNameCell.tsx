// P2 数据点名称单元格：第一行展示名（display_name ?? point_key），
// 第二行小字 point_key（仅当用户命名存在且与 key 不同时，避免重复）。
// 设备页与全局页共用，保证双行口径一致。
import type { DevicePointView } from "./useDeviceWorkspaceData";

const MONO = { fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 } as const;

export function PointNameCell({ point }: { point: DevicePointView }) {
  const key = point.key ?? point.point_key ?? String(point.point_id);
  const named = !!point.display_name?.trim() && point.displayKey !== key;
  return (
    <span>
      <span style={MONO}>{point.displayKey}</span>
      {named ? (
        <div style={{ fontSize: 11, color: "#8c8c8c", fontFamily: MONO.fontFamily }}>
          {key}
        </div>
      ) : null}
    </span>
  );
}
