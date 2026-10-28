// Browse 语义载荷处理器（handler）缝：通用 Browse 组件与「驱动专属语义」之间
// 唯一剩下的注入点。
//
// 历史：IPC 1.7 及之前，wire 上的 `BrowseNode` 只有 id/label/kind/data_type/
// access/has_children/binding_json，于是「这个叶能不能选、属于什么类别、为什么
// 不能选」只能由前端从载荷**形态**反推——PR #150 把反推集中到这里的
// `BrowseLeafPolicy` 注入点（NCK 实现见 nckBrowseSemantic.ts）。
//
// IPC 1.8 之后（`BrowseNode.selection_mode` / `selection_payload_json` /
// `diagnostics`）**可选性与阻断原因已由驱动给出**（见 browseModel.ts 的
// `evaluateBrowseNode`），所以本文件**收窄**为只承担最后一件通用组件不该做的事：
// 把**不透明的语义载荷**翻译成父层认识的类型。
//
// 收窄后的边界（明确写清，避免后人再把它扩回"判定层"）：
// - 这里**不判定**可选性：可选性只看 wire 的 `selection_mode`；
// - 这里**不生成**文案结论：阻断文案来自驱动诊断或本端契约码；
// - 这里**只做形状解析**：载荷形状不合契约必须 `ok:false`（fail-closed），
//   绝不猜、绝不补默认值、绝不把畸形载荷硬塞给父层；
// - `NckSemanticPicker` 仍是 NCK 专用表单，本缝不试图变成通用表单。
import type { BrowseBlock } from "./browseModel";

/** 语义载荷解析结果（成功给父层认识的载荷，失败给可展示的阻断原因）。 */
export type BrowseSemanticParse<TSel> =
  | { ok: true; selection: TSel; label?: string }
  | { ok: false; code: string; message: string };

/**
 * 驱动语义层的载荷处理器。
 *
 * 一个驱动语义层（如 NCK）只需实现本接口：通用组件把「驱动声明的 SEMANTIC
 * 载荷」原样交进来，把 `selection` 原样交给上层表单。
 */
export interface BrowseSemanticHandler<TSel> {
  /** 处理器标识（仅用于诊断/错误信息，不参与判定）。 */
  readonly name: string;
  /**
   * 上层未提供语义接收入口时的阻断文案。
   *
   * 由语义层给出而不是写死在通用组件里：这句话属于该语义路径的产品口径，
   * 通用组件不该知道「语义表单」这种东西。
   */
  readonly notProvidedReason?: string;
  /** 「选用」按钮文案（缺省由通用组件给出）。 */
  readonly selectLabel?: string;
  /** 解析不透明载荷；**纯函数**，不得有副作用或网络访问。 */
  parse(payload: Record<string, unknown>): BrowseSemanticParse<TSel>;
}

/** 通用组件的兜底文案（语义层未声明 notProvidedReason 时使用）。 */
export const DEFAULT_MISSING_HANDLER_REASON = "上层未提供该叶的接收入口：不可选用";

/** 上层未提供 `onSemanticSelect` 时的阻断（与"没有处理器"区分，便于定位）。 */
export function missingHandlerBlock(reason: string): BrowseBlock {
  return {
    code: "BROWSE_SEMANTIC_HANDLER_MISSING",
    message: reason,
    testId: "browse-leaf-block",
  };
}

/** 处理器拒绝载荷时的阻断（码与文案由语义层给出，通用组件原样渲染）。 */
export function parseRejectedBlock(code: string, message: string): BrowseBlock {
  return { code, message, testId: "browse-semantic-block" };
}
