// Browse 叶判定策略：通用 Browse 组件与「驱动专属语义」之间的唯一 seam。
//
// 为什么需要这一层：wire 上的 BrowseNode 只有 id/label/kind/data_type/access/
// has_children/binding_json（proto/driver.proto:340-349），**没有** `selectable`
// / `diagnostics` / 结构化 eligibility 字段。于是「这个叶能不能选用、属于什么
// 类别、为什么不能选」目前只能由驱动语义层给出。
//
// 这里把它做成**注入点**而不是在通用组件里写 if：
// - 通用组件（components/ResourceBrowseAntd.tsx）只认识本文件的类型，渲染
//   badge / 预览提示 / 阻断原因 / 选用按钮，不解释任何厂商字面量；
// - 语义实现（如 nckBrowsePolicy.ts）由调用方构造并传入，判定与文案都在那里。
//
// 纪律：
// - 策略不得放宽判定：不可选用时必须给出 reason（fail-closed 展示，不是静默隐藏）；
// - `selection` 是不透明载荷，通用组件既不解释也不改写它；
// - TODO(契约补齐)：等 BrowseNode 提供结构化字段（`selectable` /
//   `diagnostics` / 不透明 `selection`）后，本接口可退化为纯数据消费，届时
//   通用组件可以直接读节点字段。**在那之前它只是前端注入点**，不代表 wire
//   契约已经具备这些字段——不要据此声称“已完全解耦”。
import type { BrowseNode } from "./browseModel";

/** 策略给出的展示素材（全部是字符串/结构，不参与判定）。 */
export interface BrowseLeafDisplay {
  /** 叶类别标签（如「语义点位」）；通用组件原样渲染成 Tag。 */
  badge?: string;
  /** 附加标签（原样展示，如点位 kind）。 */
  tags?: readonly string[];
  /**
   * 仅展示的身份/预览提示。
   * - muted：常规提示；warn：需要用户注意（例如身份尚未确定）。
   */
  preview?: { text: string; tone: "muted" | "warn" };
  /**
   * 与 reason 独立的阻断说明：表示「该形态一律不可经 Browse 选用」，
   * 不是某次判定失败（例如降级载荷只能走别的入口）。
   */
  notice?: { text: string; testId: string };
}

/**
 * 单个叶的判定结果。
 *
 * - `generic`：策略不接管，通用资源叶规则（browseModel.selectableFromNode）生效；
 * - `owned`：策略接管该叶。`selectable=false` 时通用组件只展示原因与 notice，
 *   不提供「选用」入口；`selectable=true` 时必须同时给出 `selection`。
 */
export type BrowseLeafOwned<TSel> = {
  mode: "owned";
  selectable: boolean;
  /** selectable=false 时的具体原因（原样展示在阻断块里）。 */
  reason?: string;
  /** selectable=true 时交给父层的**不透明**载荷。 */
  selection?: TSel;
  /** 「选用」按钮文案；缺省由通用组件给出。 */
  selectLabel?: string;
  /** 阻断块的 data-testid（便于既有测试与定位）；缺省 "browse-leaf-block"。 */
  blockTestId?: string;
} & BrowseLeafDisplay;

export type BrowseLeafVerdict<TSel> = { mode: "generic" } | BrowseLeafOwned<TSel>;

/** 驱动语义层提供的 Browse 叶策略。 */
export interface BrowseLeafPolicy<TSel> {
  /** 策略标识（仅用于诊断/错误信息，不参与判定）。 */
  readonly name: string;
  /**
   * 策略层未提供「选用」接收入口时的阻断文案。
   *
   * 由策略给出而不是写死在通用组件里：这句话属于该语义路径的产品口径，
   * 通用组件不该知道「语义表单」这种东西。
   */
  readonly missingHandlerReason?: string;
  /** 判定一个节点；不接管时返回 `{ mode: "generic" }`。 */
  evaluate(node: BrowseNode): BrowseLeafVerdict<TSel>;
}

/** 通用组件的兜底文案（策略未声明 missingHandlerReason 时使用）。 */
export const DEFAULT_MISSING_HANDLER_REASON = "上层未提供该叶的接收入口：不可选用";

/** 通用组件的兜底阻断文案：载荷已声明 point kind，但没有策略接管。 */
export const NO_POLICY_FOR_POINT_KIND_REASON =
  "该叶载荷需要驱动语义层解释，当前入口未注入策略：不可选用";
