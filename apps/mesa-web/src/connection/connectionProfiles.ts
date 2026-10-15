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
 *
 * 注意：本函数只回答「哪些 key 不再由当前预设控制」，**不回答**「这些 key 该不该
 * 真的删掉」——是否删除还要看当前值是否仍等于预设写入的值（用户手工改过的值
 * 属于用户输入，不得删除）。该判定需要调用方持有「预设来源状态」，见
 * `ConnectionSetupPanel` 的 `AppliedPreset`。
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

/**
 * 「该系列没有经过 Mesa 实机验证的 TSAP 预设」的界面说明。
 *
 * 为什么用「按 family 查表」而不是在面板里写 `if (family === "840d-sl")`：
 * 「哪些系列有预设、哪些没有」本来就由本模块定义，说明文案必须与
 * [`selectablePresets`] 同源，否则将来新增预设时会出现「预设卡片与
 * 『暂无预设』说明同时出现」的自相矛盾。
 *
 * 纪律（评审明确）：没有实测依据的系列**只给说明**——不提供任何可点击的预设、
 * 也不自动填充其 TSAP。即使按钮标注「未验证」，一个可点击的示例仍会被用户
 * 读成推荐配置，因此这里连按钮都不给。
 */
const UNVERIFIED_FAMILY_NOTES: Record<string, string> = {
  "840d-sl": "840D sl 暂无经过 Mesa 实机验证的 TSAP 预设，请依据实际控制器配置填写。",
};

/**
 * 取某驱动/系列的「无实机验证预设」说明；`null` = 不需要展示。
 *
 * 返回非 null 的充要条件：驱动是 SINUMERIK NCK、系列已明确、该系列**没有**可选
 * 预设（有预设时由预设卡片自己说明来源与限制），且本模块为该系列备有说明文案。
 * 系列未选择时返回 `null`：还不知道是哪个系列，不该提前给结论。
 */
export function unverifiedFamilyNote(
  driverId: string,
  family: string | null,
): string | null {
  if (driverId !== "sinumerik-nck" || !family) return null;
  if (selectablePresets(driverId, family).length > 0) return null;
  return UNVERIFIED_FAMILY_NOTES[family] ?? null;
}
