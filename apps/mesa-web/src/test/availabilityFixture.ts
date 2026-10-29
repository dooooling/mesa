// 测试夹具：sidecar `availability` 的标准取值（仅测试用）。
//
// 为什么集中定义：`availability` 缺失 ⇒ 门禁 fail-closed（`definitionMeta.evaluateAvailabilityGate`），
// 因此**所有"应该能加点位"的既有夹具都必须显式声明可用性**——否则用例会以
// "按钮被禁用"的形式失败，而失败原因与它真正要验证的东西无关。
//
// 这里默认取 `candidate`（与真实目录分布一致：30 条 candidate / 0 条 ready），
// 让既有流程用例跑在"允许添加但带未验证标注"的真实形态上；需要 `ready` 的用例
// 显式覆盖即可。
import type { NckAvailabilityMeta } from "../nckCatalog";

export const CANDIDATE_AVAILABILITY: NckAvailabilityMeta = {
  state: "candidate",
  basis: ["no_device_read"],
  reason_zh: null,
};

/** `ready` 形态（仅用于需要"已核实可用"的用例；真实数据当前 0 条）。 */
export const READY_AVAILABILITY: NckAvailabilityMeta = {
  state: "ready",
  basis: ["manual_verified"],
  reason_zh: null,
};
