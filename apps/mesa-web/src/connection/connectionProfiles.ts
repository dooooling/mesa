/**
 * 连接配置预设（C2）。
 *
 * 纪律（V1.1 §5.2/§5.3）：
 * - 预设分三级：`verified_scope`（官方规定或多设备证据，可自动预填）、
 *   `site_tested_example`（单台/有限配置实测，**只作可选示例**，默认不静默启用）、
 *   `unverified`（没有证据，**不提供推荐值**）；
 * - 不得把单台 828D 的实测 TSAP 当成所有 828D 的通用默认；
 * - 不得在代码里偷偷使用 `0x0100/0x0102` 之类的猜测值；
 * - 预设只填 TSAP 这类实例/端口参数，不替用户决定 Host。
 */

export type PresetScope = "verified_scope" | "site_tested_example" | "unverified";

export interface ConnectionPreset {
  id: string;
  driverId: string;
  /** 适用范围标签，必须展示给用户（如 `828d`） */
  family: string;
  scope: PresetScope;
  /** 预填字段（仅限 TSAP 等；**不含 Host**） */
  values: Record<string, number>;
  /** 来源与限制说明，界面必须可见 */
  provenance: string;
  /** 该预设的证据/限制提示（保存时提示"已使用非通用示例"） */
  caveat: string;
}

/**
 * 唯一一条 `site_tested_example`。
 *
 * 来源：`docs/evidence/nck-local-migration-2026-10-13.md` 与
 * `drivers/sinumerik-nck/tests/live_identity_probe.rs` —— 单台实体 828D
 * 使用 `local_tsap=1024 (0x0400)`、`remote_tsap=3332 (0x0D04)`、`port=102`
 * 成功完成只读采样。
 *
 * 明确不是 828D 通用默认：不同软件版本/网络配置可能不同，必须由用户确认后
 * 显式应用（`scope !== "verified_scope"` ⇒ 不自动预填）。
 */
export const NCK_828D_SITE_TESTED: ConnectionPreset = {
  id: "nck.828d.site-tested-2026-10-13",
  driverId: "sinumerik-nck",
  family: "828d",
  scope: "site_tested_example",
  values: { local_tsap: 1024, remote_tsap: 3332 },
  provenance:
    "单台实体 828D 实测（local_tsap=1024/0x0400，remote_tsap=3332/0x0D04，port=102）；" +
    "证据：docs/evidence/nck-local-migration-2026-10-13.md、drivers/sinumerik-nck/tests/live_identity_probe.rs",
  caveat:
    "这是**单台设备的实测示例**，不是所有 828D 的通用默认；请与当前 CNC 的实际配置核对后再使用。",
};

const PRESETS: ConnectionPreset[] = [NCK_828D_SITE_TESTED];

/** 某驱动在指定系列下**可自动预填**的预设（仅 `verified_scope`）。 */
export function autoApplicablePreset(
  driverId: string,
  family: string | null,
): ConnectionPreset | null {
  if (!family) return null;
  return (
    PRESETS.find(
      (p) =>
        p.driverId === driverId &&
        p.family === family &&
        p.scope === "verified_scope",
    ) ?? null
  );
}

/** 某驱动在指定系列下**可选应用**的示例预设（含 `site_tested_example`）。 */
export function selectablePresets(
  driverId: string,
  family: string | null,
): ConnectionPreset[] {
  if (!family) return [];
  return PRESETS.filter(
    (p) => p.driverId === driverId && p.family === family && p.scope !== "unverified",
  );
}

/**
 * 切换系列/驱动时必须清除不再适用的预填值。
 *
 * 返回需要从 raw 中**删除**的字段名：只清理由预设写入的键，不动用户手填的
 * 其它字段（避免误删）。
 */
export function fieldsToClearOnSwitch(
  previous: ConnectionPreset | null,
  next: ConnectionPreset | null,
): string[] {
  if (!previous) return [];
  if (next && next.id === previous.id) return [];
  const nextKeys = new Set(Object.keys(next?.values ?? {}));
  return Object.keys(previous.values).filter((k) => !nextKeys.has(k));
}
