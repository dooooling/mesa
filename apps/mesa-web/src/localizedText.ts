// 统一的 LocalizedText 取值（纯展示层，不 import React/AntD）。
//
// 契约背景（core-types `LocalizedText`）：`default` 必有，`"zh-CN"` 可选。
// 规则只有一条：**优先 zh-CN，回落 default**；两者都不可用时才用调用方兜底。
//
// 为什么集中成一个函数：此前只有事件编辑器（EventTaskEditor.streamLabel）
// 读了 `zh-CN`，资源选择器仍渲染 `default`，于是同一份 Descriptor 在「事件订阅」
// 显示中文、在「资源选型」显示英文。取值规则一旦分叉，后续每加一个渲染点都会
// 再分叉一次，因此这里收敛为唯一实现。
import type { LocalizedText } from "./types";

/**
 * 取本地化文本：非空 `zh-CN` → 非空 `default` → `fallback`。
 *
 * 「非空」而非「存在」：空串是「没有译文」的数据形态，若把它当权威值，
 * 界面会出现空白标签（比显示英文更难排查）。`fallback` 由调用方给
 * （通常是 id / key），本函数不猜任何业务含义。
 */
export function localizedText(
  label: LocalizedText | null | undefined,
  fallback = "",
): string {
  if (!label) return fallback;
  const zh = label["zh-CN"];
  if (typeof zh === "string" && zh.trim() !== "") return zh;
  const def = label.default;
  if (typeof def === "string" && def.trim() !== "") return def;
  return fallback;
}
