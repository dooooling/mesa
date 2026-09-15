// P2 数据点名称单元格：第一行展示名（display_name ?? point_key），
// 第二行小字 point_key（仅当用户命名存在且与 key 不同时，避免重复）。
// 设备页与全局页共用，保证双行口径一致。
// 只收 primitive props（memo 身份稳定，timestamp 变化不连带重渲染）。
const MONO = { fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 } as const;

export function PointNameCell({
  displayKey,
  pointKey,
  displayName,
}: {
  displayKey: string;
  pointKey: string;
  displayName?: string;
}) {
  const named = !!displayName?.trim() && displayKey !== pointKey;
  return (
    <span>
      <span style={MONO}>{displayKey}</span>
      {named ? (
        <div style={{ fontSize: 11, color: "#8c8c8c", fontFamily: MONO.fontFamily }}>
          {pointKey}
        </div>
      ) : null}
    </span>
  );
}
