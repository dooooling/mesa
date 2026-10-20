// PR-D1 质量码解释（唯一语义边界，P0-3）。
//
// 为什么必须带 `driver_id`：`quality_code` 是**异构十进制字符串**——
// sinumerik-nck 上是 S7 item 返回码字节、opcua 上是 `StatusCode` 位模式
// （`u32 as i32`，可为负）、focas2 上是 Mesa 自己的错误分类码。同一个数字
// `3` 在 nck 是 S7 参考码表的访问拒绝类、在 focas2 是"参数类"，语义完全不同。
// 因此分类/统计的最小键是 `(driver_id, code)` 二元组，跨驱动不得相加同名数字。
//
// 判据只有一条：**仓库内是否存在该码的显式定义**。有定义才给一句话解释；
// 否则只显示原值（`UNKNOWN`，不解释）。绝不猜厂商码表。
//
// 文案红线（§4.4）：不得把 Core 写的符号标记说成设备返回码；不得由 `0x05`
// 直接断言"地址不存在"等具体根因（仓库码表声明的是 CPU 侧语义，与 item 级
// 字节是否同义**尚未确认**）。
import type { TagProps } from "antd";

/** 码的命名空间分类（§4.2 判定表的输出）。 */
export type CodeClass =
  | "MESA_MARKER"
  | "DEVICE_S7_ITEM"
  | "PROTOCOL_STATUS"
  | "MESA_CLASS"
  | "UNKNOWN"
  | "NONE";

/** Core 写出的符号标记：**不是**设备返回码（`snapshot.rs:318-324`、`:405`）。 */
export const MESA_MARKER_DEVICE_ERROR = "DEVICE_ERROR";
export const MESA_MARKER_COMMUNICATION_LOST = "COMMUNICATION_LOST";

/** 断线覆盖提示（U3）：此前的项级码已被 `COMMUNICATION_LOST` 覆盖，不可恢复。 */
export const COMMUNICATION_LOST_NOTE = "此前的项级码已被覆盖，不可恢复";

/**
 * 非 GOOD 行缺码的固定文案（§4.2 逐字）。
 *
 * 为什么必须显示：兜底逻辑本应恒写 `DEVICE_ERROR`（`snapshot.rs:318-324`），
 * 缺失本身就是异常形态；若像 GOOD 行一样隐藏，操作员会把"没有原因码"
 * 误读成"没有异常"，直接违背 D1 让故障原因可见的目的。
 */
export const MISSING_CODE_TEXT = "未提供（异常形态，请报告）";

/** 码区固定脚注（§4.4，逐字）。 */
export const CODE_FOOTNOTE = "展示一个码 ≠ 确定根因；码表仅覆盖仓库内已定义的部分。";

/** 展示一个码 ≠ 确定根因：所有码区底部都必须带这句。 */
const S7_REFERENCE_TEXT: Record<number, string> = {
  // 3/4/5 只给**类别名**（首版收窄口径 P1-3）：仓库码表是 CPU 侧语义，
  // 与 item 级返回字节是否逐字同义尚未确认（§12-2）。
  3: "仓库现有 S7 参考码表：访问拒绝类（可能关联，需结合协议层确认）",
  4: "仓库现有 S7 参考码表：上下文不支持类（可能关联，需结合协议层确认）",
  5: "仓库现有 S7 参考码表：地址错误类（可能关联，需结合协议层确认）",
};

/** FOCAS2 的 `1..5` 是 Mesa 单点错误分类码（非厂家码）。 */
const FOCAS_CLASS_TEXT: Record<number, string> = {
  1: "Mesa 分类：其他/未知",
  2: "Mesa 分类：不支持/未实现（EW_NOOPT 等）",
  3: "Mesa 分类：参数类（EW_PARAM/RANGE/NUMBER/LENGTH）",
  4: "Mesa 分类：数据/属性类（EW_DATA/ATTRIB）",
  5: "Mesa 分类：连接类（SOCKET/HANDLE/BUSY/NODLL/CLOSED/TIMEOUT）",
};

function normalizeDriverId(driverId: string | null | undefined): string {
  return (driverId ?? "").trim().toLowerCase();
}

/**
 * 十进制字符串 → 整数。`quality_code` 线上恒为 `i32::to_string()` 或符号标记，
 * 非十进制（含符号标记、空串）一律返回 null（不解释）。
 */
function parseDecimalCode(code: string | undefined): number | null {
  if (code === undefined) return null;
  const s = code.trim();
  if (!/^-?\d+$/.test(s)) return null;
  const n = Number(s);
  return Number.isSafeInteger(n) ? n : null;
}

function isMesaMarker(code: string): boolean {
  return code === MESA_MARKER_DEVICE_ERROR || code === MESA_MARKER_COMMUNICATION_LOST;
}

/**
 * 按 `(driver_id, code)` 判定命名空间分类。
 *
 * - 码缺失：`quality="GOOD"` 时是正常形态（`NONE`，不渲染码区）；
 *   否则是异常形态（兜底逻辑应恒写 `DEVICE_ERROR`），归 `UNKNOWN`。
 * - 符号标记恒为 `MESA_MARKER`（与 driver 无关）。
 * - 只有 nck / opcua / focas2 有已知语义空间；其他 driver（含空 driver）
 *   一律 `UNKNOWN`——没有驱动信息就不解释。
 */
export function classifyCode(
  driverId: string | null | undefined,
  code: string | undefined,
  quality?: string,
): CodeClass {
  if (code === undefined) {
    return (quality ?? "").toUpperCase() === "GOOD" ? "NONE" : "UNKNOWN";
  }
  if (isMesaMarker(code)) return "MESA_MARKER";
  const n = parseDecimalCode(code);
  if (n === null) return "UNKNOWN";
  const driver = normalizeDriverId(driverId);
  if (driver === "sinumerik-nck") {
    // S7 item 返回码是设备原始字节（0xFF 被驱动过滤）。
    return n >= 0 && n <= 255 ? "DEVICE_S7_ITEM" : "UNKNOWN";
  }
  if (driver === "opcua") return "PROTOCOL_STATUS";
  if (driver === "focas2") return n >= 1 && n <= 5 ? "MESA_CLASS" : "UNKNOWN";
  return "UNKNOWN";
}

/**
 * 码区是否渲染（U2 与 §4.2 的裁决）：
 * - 有码 ⇒ 恒渲染；
 * - 无码 + `GOOD` ⇒ 隐藏（GOOD 行零额外 DOM，T7 依赖）；
 * - 无码 + 非 `GOOD` ⇒ **必须渲染**并显示 {@link MISSING_CODE_TEXT}
 *   （缺失即异常形态，不得静默隐藏）。
 */
export function shouldShowCodeSection(
  quality: string | undefined,
  code: string | undefined,
): boolean {
  if (code !== undefined) return true;
  return (quality ?? "").toUpperCase() !== "GOOD";
}

/**
 * 一句话解释；无可解释定义时返回 `null`（UI 只显示原值）。
 * 符号标记的解释**不得**含"设备"字样——它们是 Core 本地标记。
 */
export function codeText(
  driverId: string | null | undefined,
  code: string | undefined,
): string | null {
  if (code === undefined) return null;
  const cls = classifyCode(driverId, code);
  if (cls === "MESA_MARKER") {
    return code === MESA_MARKER_DEVICE_ERROR
      ? "驱动未提供数字码"
      : "连接丢失，已覆盖该点此前的码";
  }
  const n = parseDecimalCode(code);
  if (n === null) return null;
  if (cls === "DEVICE_S7_ITEM") return S7_REFERENCE_TEXT[n] ?? null;
  if (cls === "MESA_CLASS") return FOCAS_CLASS_TEXT[n] ?? null;
  if (cls === "PROTOCOL_STATUS") return "OPC UA 状态码（位模式），无仓库码表";
  return null;
}

/**
 * 十六进制视图（只加视图，不改 REST 的十进制字符串格式）：
 * `"5"` → `0x05 (5)`；负值按 `u32` 位模式展示：`"-2143289344"` → `0x80340000 (-2143289344)`。
 * 非十进制码（符号标记）返回 null，由调用方原样显示。
 */
export function codeHex(code: string | undefined): string | null {
  const n = parseDecimalCode(code);
  if (n === null) return null;
  let hex: string;
  if (n < 0) {
    hex = (n >>> 0).toString(16).toUpperCase().padStart(8, "0");
  } else {
    const width = n <= 0xff ? 2 : n <= 0xffff ? 4 : 8;
    hex = n.toString(16).toUpperCase().padStart(width, "0");
  }
  return `0x${hex} (${n})`;
}

/** 命名空间标签（U4/U5）：UI 必须把码与其命名空间绑定显示。 */
export function namespaceLabel(cls: CodeClass): string | null {
  if (cls === "DEVICE_S7_ITEM") return "S7 item";
  if (cls === "PROTOCOL_STATUS") return "OPC UA StatusCode";
  if (cls === "MESA_CLASS") return "Mesa 分类";
  if (cls === "MESA_MARKER") return "Mesa 标记";
  if (cls === "UNKNOWN") return "未知命名空间";
  return null;
}

/**
 * 分区标题（U3/U4/U5、统计 Popover 同口径）。
 *
 * 缺陷 2（语义错误）：FOCAS2 的 `1..5` 是 **Mesa 自己的单点错误分类码**，
 * 不是厂家/设备返回码（§4.1、§4.2 明确写着"非厂家码"）。因此 `MESA_CLASS`
 * 必须单独成组，**不得**与 S7/NCK 的 `DEVICE_S7_ITEM`、OPC UA 的
 * `PROTOCOL_STATUS` 同组展示——同组会让读者把它当成设备给出的码。
 */
export function codeGroupTitle(cls: CodeClass): string {
  if (cls === "MESA_MARKER") return "Mesa 标记（不是设备返回码）";
  if (cls === "DEVICE_S7_ITEM" || cls === "PROTOCOL_STATUS") {
    return "设备/协议返回码";
  }
  if (cls === "MESA_CLASS") return "Mesa 分类（非厂家码）";
  if (cls === "UNKNOWN") return "未知命名空间（不解释）";
  return "";
}

// ---------------------------------------------------------------------------
// 采样状态（§5.3）：`value_origin` 表达"值的来源"，`quality` 表达"质量"，
// 两者不可相互替代。`CURRENT` 只说明值由本次 attempt 产生，**不说明采集成功**
// ——`CURRENT + BAD` 是"本次尝试得到的最新一次失败结果"，必须用红色，
// 不得用成功绿（P1-1）。
// ---------------------------------------------------------------------------

export type Freshness = "CURRENT" | "STALE_VALUE" | "NO_VALUE" | "UNKNOWN";

/** 新鲜度只看 `value_origin`，不看端点 `state`（RUNNING 不代表数据新鲜）。 */
export function deriveFreshness(valueOrigin: string | undefined): Freshness {
  if (valueOrigin === "CURRENT") return "CURRENT";
  if (valueOrigin === "LAST_KNOWN") return "STALE_VALUE";
  if (valueOrigin === "PLACEHOLDER") return "NO_VALUE";
  return "UNKNOWN";
}

export interface SamplingBadge {
  /** 徽标文案（§5.3 逐条件）。 */
  text: string;
  /** 徽标颜色：CURRENT+GOOD 绿；CURRENT+BAD 红；其余灰（`default`）。 */
  color: TagProps["color"];
  /** 是否"非本次 attempt 产生"（时间列追加限定语）。 */
  notCurrent: boolean;
  /** 是否显示"不可恢复"提示（断线覆盖）。 */
  unrecoverable: boolean;
  /** 理论不可达的异常组合（如 CURRENT+COMMUNICATION_LOST）需要标注。 */
  abnormal: boolean;
}

/**
 * 按 `(value_origin, quality, codeClass)` 取 §5.3 的徽标。
 *
 * 理论不可达组合（`mark_communication_lost` 会把 `CURRENT`→`LAST_KNOWN`，
 * 见 `snapshot.rs:406-408`）若真出现，按失败渲染并标"异常形态"——宁可显示
 * 异常，也不静默把它当正常。
 */
export function samplingBadge(
  valueOrigin: string | undefined,
  quality: string,
  code?: string,
): SamplingBadge {
  const q = (quality ?? "").toUpperCase();
  const freshness = deriveFreshness(valueOrigin);
  // "断线"文案只认 COMMUNICATION_LOST 这个标记本身，不把所有 Mesa 标记当断线。
  const lost = code === MESA_MARKER_COMMUNICATION_LOST;
  if (freshness === "CURRENT") {
    if (q === "BAD") {
      return {
        text:
          code === MESA_MARKER_DEVICE_ERROR
            ? "本次返回失败（未提供数字码）"
            : "本次返回失败",
        color: "red",
        notCurrent: false,
        unrecoverable: false,
        // CURRENT + COMMUNICATION_LOST 理论不可达（断线会把 CURRENT 降为
        // LAST_KNOWN）；真出现则标注异常形态，不静默当正常。
        abnormal: lost,
      };
    }
    if (q === "UNCERTAIN") {
      return {
        text: "本次返回不确定",
        color: "orange",
        notCurrent: false,
        unrecoverable: false,
        abnormal: false,
      };
    }
    return {
      text: "当前有效值",
      color: "green",
      notCurrent: false,
      unrecoverable: false,
      abnormal: false,
    };
  }
  if (freshness === "STALE_VALUE") {
    return {
      text: lost ? "旧值，未更新（断线前）" : "旧值，未更新（本次 attempt 未更新该点）",
      color: "default",
      notCurrent: true,
      unrecoverable: lost,
      abnormal: false,
    };
  }
  if (freshness === "NO_VALUE") {
    return {
      text: lost ? "无有效值（且已断线）" : "无有效值",
      color: "default",
      notCurrent: true,
      unrecoverable: lost,
      abnormal: false,
    };
  }
  // value_origin 缺失：服务端恒发该字段，缺失即异常形态，不猜来源。
  return {
    text: "采样状态未知（服务端未提供 value_origin）",
    color: "default",
    notCurrent: true,
    unrecoverable: false,
    abnormal: true,
  };
}
