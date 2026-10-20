// PR-D1 原始快照导出（T10–T15）。
//
// 证据分级（P1-2 / P0-2）：
// - **原始证据** = `points_raw_text` + `evidence_integrity`（UTF-8 字节级 SHA-256）；
// - **结构化副本** = `points[]` + `summary`（允许精度损失，不作为对账依据）；
// - 时间戳一律**十进制字符串**（BigInt），因为 Unix ns ≈1.8e18 超出
//   `Number.MAX_SAFE_INTEGER`，number 会静默舍入（§6.6-1）。
// 请求上界：一次点击 **4 个既有 GET**（2 波），任一失败即中止且**不产文件**。
import { describe, expect, it } from "vitest";
import { buildPointsExport } from "./qualityExport";

const EP_BEFORE = [
  {
    id: "lx-nck",
    name: "NCK",
    driver_id: "sinumerik-nck",
    device_id: "cnc-01",
    connection: { host: "10.0.0.5" },
    runtime: { state: "RUNNING", detail: "", revision: 16, points: 2, epoch: 123 },
  },
];

const ROW_CURRENT = {
  endpoint_id: "lx-nck",
  point_id: 261,
  point_key: "native.C.1.127.13.1",
  key: "native.C.1.127.13.1",
  type: "u32",
  value: 5,
  quality: "GOOD",
  timestamp_ns: 1792460055540482200,
  value_origin: "CURRENT",
};

const ROW_LAST_KNOWN = {
  endpoint_id: "lx-nck",
  point_id: 445,
  point_key: "native.A.1.26.30110.1",
  key: "native.A.1.26.30110.1",
  type: "u32",
  value: 0,
  quality: "BAD",
  quality_code: "COMMUNICATION_LOST",
  timestamp_ns: 1792460055540482200,
  value_origin: "LAST_KNOWN",
};

const LATEST = { points: [ROW_CURRENT, ROW_LAST_KNOWN], count: 2 };
const LATEST_TEXT = JSON.stringify(LATEST);

function jsonResponse(body: unknown) {
  const text = JSON.stringify(body);
  return {
    ok: true,
    status: 200,
    text: async () => text,
    json: async () => JSON.parse(text) as unknown,
  };
}

function textResponse(text: string) {
  return {
    ok: true,
    status: 200,
    text: async () => text,
    json: async () => JSON.parse(text) as unknown,
  };
}

/** fetch 桩：记录调用顺序；可指定第 N 次调用失败（1-based）。 */
function makeFetch(opts: {
  before?: unknown[];
  after?: unknown[];
  version?: string;
  latestText?: string;
  failAt?: number;
}) {
  const calls: string[] = [];
  let endpointsCalls = 0;
  const fetchImpl = (async (url: string) => {
    calls.push(url);
    if (opts.failAt !== undefined && calls.length === opts.failAt) {
      return { ok: false, status: 500, text: async () => "", json: async () => ({}) };
    }
    if (url === "/api/v1/endpoints") {
      endpointsCalls += 1;
      return jsonResponse({
        endpoints: endpointsCalls === 1 ? (opts.before ?? EP_BEFORE) : (opts.after ?? opts.before ?? EP_BEFORE),
      });
    }
    if (url === "/api/v1/diagnostics") {
      return jsonResponse({ version: opts.version ?? "0.0.0" });
    }
    if (url === "/api/v1/points/latest") {
      return textResponse(opts.latestText ?? LATEST_TEXT);
    }
    throw new Error(`unexpected url: ${url}`);
  }) as unknown as typeof fetch;
  return { fetchImpl, calls };
}

/** 递归收集全部对象键（T12 断言"不存在有效性判据字段"用）。 */
function collectKeys(value: unknown, out = new Set<string>()): Set<string> {
  if (Array.isArray(value)) {
    for (const v of value) collectKeys(v, out);
    return out;
  }
  if (value && typeof value === "object") {
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      out.add(k);
      collectKeys(v, out);
    }
  }
  return out;
}

describe("qualityExport · 原始快照导出（PR-D1）", () => {
  it("export_builds_schema_with_sampling_context", async () => {
    const { fetchImpl, calls } = makeFetch({});
    const result = await buildPointsExport({ now: () => 1_792_460_055_541, fetchImpl });
    // 4 个既有 GET，顺序固定：状态前 → 版本 → 原始快照 → 状态后。
    expect(calls).toEqual([
      "/api/v1/endpoints",
      "/api/v1/diagnostics",
      "/api/v1/points/latest",
      "/api/v1/endpoints",
    ]);
    expect(result.requestCount).toBe(4);
    expect(result.files).toHaveLength(1);

    const doc = result.document;
    expect(doc.schema).toBe("mesa.points.export.v1");
    expect(doc.mesa_version).toBe("0.0.0");
    expect(doc.history).toEqual({
      available: false,
      reason: "Snapshot 仅驻留当前最新值，不含时间序列",
    });
    expect(doc.sampling.state_consistency).toBe("STATE_UNCHANGED");
    expect(doc.sampling.request_status).toBe("COMPLETE");
    expect(doc.sampling.freshness_summary).toEqual({
      current: 1,
      last_known: 1,
      placeholder: 0,
      unknown: 0,
    });
    expect(doc.sampling.evidence_integrity.raw_preserved).toBe("INLINE");
    expect(doc.sampling.evidence_integrity.raw_sha256).toMatch(/^[0-9a-f]{64}$/);
    expect(doc.sampling.evidence_integrity.raw_utf8_bytes).toBe(
      new TextEncoder().encode(LATEST_TEXT).length,
    );
    // 采样上下文：端点状态逐字保留（含 driver_id 与 runtime 五字段）。
    expect(doc.sampling.endpoint_state_before).toEqual([
      {
        id: "lx-nck",
        driver_id: "sinumerik-nck",
        runtime: { state: "RUNNING", detail: "", revision: 16, points: 2, epoch: 123 },
      },
    ]);
    expect(doc.sampling.endpoint_state_after).toEqual(doc.sampling.endpoint_state_before);
    // 时间戳是十进制字符串（不是 number）。
    expect(doc.taken_at_ns).toMatch(/^\d+$/);
    expect(doc.sampling.points_read_at_ns).toMatch(/^\d+$/);
    // 结构化副本 == 原始响应 JSON.parse 的结果（不与原文比纳秒数值）。
    expect(doc.points).toEqual((JSON.parse(LATEST_TEXT) as { points: unknown[] }).points);
    // 汇总口径。
    expect(doc.summary.by_quality).toEqual({ GOOD: 1, UNCERTAIN: 0, BAD: 1, UNKNOWN: 0 });
    expect(doc.summary.by_code).toEqual([
      {
        endpoint_id: "lx-nck",
        driver_id: "sinumerik-nck",
        code: "COMMUNICATION_LOST",
        origin: "MESA_MARKER",
        count: 1,
      },
    ]);
    expect(doc.scope).toEqual({ device_id: null, endpoint_id: null, row_count: 2 });
    // 原文作为内联证据保留，且文件是单文件。
    expect(doc.points_raw_text).toBe(LATEST_TEXT);
    expect(result.files[0].name).toMatch(/^mesa-points-export-\d{8}-\d{6}-all\.json$/);
  });

  it("export_marks_raw_scope_vs_analysis_scope", async () => {
    // 缺陷 3：原始响应始终是全部端点（证据完整），分析只覆盖所选连接。
    // 两者必须在导出对象里显式区分，防止用户以为导出只含自己选的连接。
    const rowOther = {
      endpoint_id: "lx-other",
      point_id: 9,
      point_key: "DB1.temp",
      key: "DB1.temp",
      type: "f64",
      value: 36.5,
      quality: "BAD",
      quality_code: "DEVICE_ERROR",
      timestamp_ns: 1792460055540482200,
      value_origin: "CURRENT",
    };
    const latestText = JSON.stringify({ points: [ROW_LAST_KNOWN, rowOther], count: 2 });
    const endpoints = [
      ...EP_BEFORE,
      {
        id: "lx-other",
        name: "S7",
        driver_id: "s7",
        device_id: "plc-01",
        runtime: { state: "RUNNING", detail: "", revision: 1, points: 1, epoch: 1 },
      },
    ];
    const { fetchImpl } = makeFetch({ before: endpoints, latestText });
    const { document: doc } = await buildPointsExport({
      scope: { endpointId: "lx-nck" },
      now: () => 1_792_460_055_541,
      fetchImpl,
    });

    // 分析范围 = 所选连接。
    expect(doc.scope).toEqual({ device_id: null, endpoint_id: "lx-nck", row_count: 1 });
    expect(doc.points).toHaveLength(1);
    expect(doc.summary.by_code.map((e) => e.endpoint_id)).toEqual(["lx-nck"]);
    expect(doc.analysis_scope).toEqual({
      device_id: null,
      // 设备维度：即使没按设备筛选，也必须如实记录"实际覆盖了哪些设备"。
      device_ids: ["cnc-01"],
      endpoint_id: "lx-nck",
      row_count: 1,
      description: expect.stringContaining("所选连接"),
    });

    // 原始证据范围 = 全部端点（含其他连接/设备），原文未丢弃。
    expect(doc.raw_scope).toEqual({
      source: "GET /api/v1/points/latest",
      includes: "ALL_ENDPOINTS",
      row_count: 2,
      description: expect.stringContaining("全部端点"),
    });
    expect(doc.points_raw_text).toBe(latestText);
    expect(doc.points_raw_text).toContain("lx-other");

    // 未选连接/设备时两者一致（分析也覆盖全部设备与全部连接）。
    const { fetchImpl: fetchAll } = makeFetch({ before: endpoints, latestText });
    const { document: all } = await buildPointsExport({
      now: () => 1_792_460_055_541,
      fetchImpl: fetchAll,
    });
    expect(all.analysis_scope).toEqual({
      device_id: null,
      device_ids: ["cnc-01", "plc-01"],
      endpoint_id: null,
      row_count: 2,
      description: expect.stringContaining("全部连接"),
    });
    expect(all.raw_scope.row_count).toBe(2);
  });

  it("export_scope_device_and_connection_matrix", async () => {
    // 复审修订 1：统计/导出分析 = 设备 + 连接双重作用域。
    // 两台设备各两个连接 ⇒ 四种组合的 analysis_scope 与行集都必须正确。
    const eps = [
      { id: "ep-a1", driver_id: "s7", device_id: "dev-a", runtime: { state: "RUNNING" } },
      { id: "ep-a2", driver_id: "s7", device_id: "dev-a", runtime: { state: "RUNNING" } },
      { id: "ep-b1", driver_id: "s7", device_id: "dev-b", runtime: { state: "RUNNING" } },
      { id: "ep-b2", driver_id: "s7", device_id: "dev-b", runtime: { state: "RUNNING" } },
    ];
    const rows = ["ep-a1", "ep-a2", "ep-b1", "ep-b2"].map((ep, i) => ({
      ...ROW_CURRENT,
      endpoint_id: ep,
      point_id: 100 + i,
      point_key: `${ep}.k`,
      key: `${ep}.k`,
    }));
    const latestText = JSON.stringify({ points: rows, count: rows.length });

    const cases: Array<{
      scope: { deviceId?: string | null; endpointId?: string | null };
      rowCount: number;
      endpointIds: string[];
      deviceId: string | null;
      deviceIds: string[];
    }> = [
      // 全部设备 + 全部连接：所有设备、所有连接。
      { scope: {}, rowCount: 4, endpointIds: ["ep-a1", "ep-a2", "ep-b1", "ep-b2"], deviceId: null, deviceIds: ["dev-a", "dev-b"] },
      // 设备选中 + 全部连接：仅该设备的全部连接（不是跨设备）。
      { scope: { deviceId: "dev-a" }, rowCount: 2, endpointIds: ["ep-a1", "ep-a2"], deviceId: "dev-a", deviceIds: ["dev-a"] },
      // 设备 + 单连接：仅该连接。
      { scope: { deviceId: "dev-a", endpointId: "ep-a1" }, rowCount: 1, endpointIds: ["ep-a1"], deviceId: "dev-a", deviceIds: ["dev-a"] },
      // 全部设备 + 单连接：仅该连接。
      { scope: { endpointId: "ep-b1" }, rowCount: 1, endpointIds: ["ep-b1"], deviceId: null, deviceIds: ["dev-b"] },
    ];

    for (const c of cases) {
      const { fetchImpl } = makeFetch({ before: eps, latestText });
      const { document: doc } = await buildPointsExport({
        scope: c.scope,
        now: () => 1_792_460_055_541,
        fetchImpl,
      });
      expect(doc.analysis_scope.device_id).toBe(c.deviceId);
      expect(doc.analysis_scope.endpoint_id).toBe(c.scope.endpointId ?? null);
      expect(doc.analysis_scope.device_ids).toEqual(c.deviceIds);
      expect(doc.analysis_scope.row_count).toBe(c.rowCount);
      expect(doc.scope).toEqual({
        device_id: c.deviceId,
        endpoint_id: c.scope.endpointId ?? null,
        row_count: c.rowCount,
      });
      // 结构化副本/汇总只覆盖作用域内的行。
      expect(doc.points.map((p) => p.endpoint_id).sort()).toEqual([...c.endpointIds].sort());
      expect(doc.sampling.freshness_summary.current).toBe(c.rowCount);
      expect(doc.summary.by_quality.GOOD).toBe(c.rowCount);
      // 原始证据始终是全部端点，绝不被作用域裁剪。
      expect(doc.raw_scope.row_count).toBe(4);
      expect(doc.raw_scope.includes).toBe("ALL_ENDPOINTS");
      expect(doc.points_raw_text).toBe(latestText);
    }
  });

  it("export_device_scope_fail_closed_for_unattributed_points", async () => {
    // 端点不在 /endpoints 清单里 ⇒ 归属不可证明 ⇒ 设备筛选下必须排除
    //（与表格 fail-closed 同口径），不得猜成"可能属于所选设备"。
    const rows = [
      { ...ROW_CURRENT, endpoint_id: "ep-a1", point_id: 1 },
      { ...ROW_CURRENT, endpoint_id: "ghost", point_id: 2 },
    ];
    const latestText = JSON.stringify({ points: rows, count: 2 });
    const { fetchImpl } = makeFetch({
      before: [
        { id: "ep-a1", driver_id: "s7", device_id: "dev-a", runtime: { state: "RUNNING" } },
      ],
      latestText,
    });
    const { document: doc } = await buildPointsExport({
      scope: { deviceId: "dev-a" },
      now: () => 1_792_460_055_541,
      fetchImpl,
    });
    expect(doc.points.map((p) => p.endpoint_id)).toEqual(["ep-a1"]);
    expect(doc.analysis_scope.row_count).toBe(1);
    expect(doc.analysis_scope.device_ids).toEqual(["dev-a"]);
    // 原始证据仍然包含 ghost 行（证据完整，不被分析范围裁剪）。
    expect(doc.raw_scope.row_count).toBe(2);
  });

  it("export_filename_reflects_device_scope", async () => {
    // 文件名是分享时最先看到的东西：设备筛选必须体现在名字里，
    // 否则"只含所选设备"的分析文件会被误当成整库快照。
    const { fetchImpl } = makeFetch({});
    const { files } = await buildPointsExport({
      scope: { deviceId: "cnc-01" },
      now: () => 1_792_460_055_541,
      fetchImpl,
    });
    expect(files[0].name).toMatch(/^mesa-points-export-\d{8}-\d{6}-cnc-01\.json$/);

    const { fetchImpl: f2 } = makeFetch({});
    const { files: both } = await buildPointsExport({
      scope: { deviceId: "cnc-01", endpointId: "lx-nck" },
      now: () => 1_792_460_055_541,
      fetchImpl: f2,
    });
    expect(both[0].name).toMatch(/^mesa-points-export-\d{8}-\d{6}-cnc-01__lx-nck\.json$/);
  });

  it("export_counts_unknown_value_origin_separately", async () => {
    // 缺陷 4：未知/缺失 value_origin 不得并入 PLACEHOLDER（会静默抬高"无有效值"）。
    const base = { ...ROW_CURRENT, quality: "BAD" };
    const rows = [
      { ...base, point_id: 1, value_origin: "CURRENT" },
      { ...base, point_id: 2, value_origin: "LAST_KNOWN" },
      { ...base, point_id: 3, value_origin: "PLACEHOLDER" },
      { ...base, point_id: 4, value_origin: "FUTURE_ORIGIN" },
      { ...base, point_id: 5, value_origin: undefined },
    ];
    const latestText = JSON.stringify({ points: rows, count: rows.length });
    const { fetchImpl } = makeFetch({ latestText });
    const { document: doc } = await buildPointsExport({
      now: () => 1_792_460_055_541,
      fetchImpl,
    });
    const f = doc.sampling.freshness_summary;
    expect(f).toEqual({ current: 1, last_known: 1, placeholder: 1, unknown: 2 });
    // 未识别来源单独计数，不得静默合并。
    expect(f.placeholder).toBe(1);
    expect(f.current + f.last_known + f.placeholder + f.unknown).toBe(doc.scope.row_count);
  });

  it("export_aborts_on_http_error", async () => {
    // 四个请求**任一**失败都必须中止，且不产生任何文件（含双文件模式）。
    for (const failAt of [1, 2, 3, 4]) {
      const { fetchImpl, calls } = makeFetch({ failAt });
      let files: unknown = null;
      let error: unknown = null;
      try {
        const result = await buildPointsExport({ now: () => 1_792_460_055_541, fetchImpl });
        files = result.files;
      } catch (e) {
        error = e;
      }
      expect(error).toBeInstanceOf(Error);
      expect((error as Error).message).toContain("500");
      expect(files).toBeNull();
      // 失败后不得继续发后续请求（第一波并行，故失败点 1 时已发出 2 个）。
      expect(calls.length).toBeLessThanOrEqual(4);
      expect(calls.length).toBeGreaterThanOrEqual(failAt);
      if (failAt < 3) {
        expect(calls).not.toContain("/api/v1/points/latest");
      }
    }
  });

  it("export_separates_state_consistency_from_freshness", async () => {
    const changed = [
      {
        id: "lx-nck",
        name: "NCK",
        driver_id: "sinumerik-nck",
        device_id: "cnc-01",
        runtime: { state: "RECONNECTING", detail: "", revision: 17, points: 2, epoch: 124 },
      },
    ];
    const { fetchImpl } = makeFetch({ before: EP_BEFORE, after: changed });
    const result = await buildPointsExport({
      now: () => 1_792_460_055_541,
      fetchImpl,
    });
    const doc = result.document;
    // 状态漂移只影响 state_consistency，不产生任何"有效性"判据。
    expect(doc.sampling.state_consistency).toBe("STATE_CHANGED_DURING_READ");
    const keys = collectKeys(doc);
    expect(keys.has("sample_status")).toBe(false);
    expect(keys.has("VALID")).toBe(false);
    expect(keys.has("FAILED")).toBe(false);
    // freshness 是来源分布，三项之和 == scope.row_count（每点 1 样本）。
    const f = doc.sampling.freshness_summary;
    expect(f.current + f.last_known + f.placeholder).toBe(doc.scope.row_count);
    expect(doc.scope.row_count).toBe(2);
    // 文件名追加 -statechanged（U10）。
    expect(result.files[0].name).toContain("-statechanged");
    expect(doc.sampling.request_status).toBe("COMPLETE");
  });

  it("export_separates_raw_when_large", async () => {
    // >5 MiB 原文：不丢弃原文，拆成 -raw.json + -analysis.json，两边 sha256 相同。
    const huge = "x".repeat(5 * 1024 * 1024 + 1024);
    const bigText = JSON.stringify({
      points: [{ ...ROW_CURRENT, value: huge }],
      count: 1,
    });
    expect(bigText.length).toBeGreaterThan(5 * 1024 * 1024);
    const { fetchImpl } = makeFetch({ latestText: bigText });
    const result = await buildPointsExport({ now: () => 1_792_460_055_541, fetchImpl });
    const evidence = result.document.sampling.evidence_integrity;
    expect(evidence.raw_preserved).toBe("SEPARATE_FILE");
    expect(evidence.raw_utf8_bytes).toBe(new TextEncoder().encode(bigText).length);
    expect(result.files).toHaveLength(2);
    const rawFile = result.files.find((f) => f.name.endsWith("-raw.json"));
    const analysisFile = result.files.find((f) => f.name.endsWith("-analysis.json"));
    expect(rawFile).toBeTruthy();
    expect(analysisFile).toBeTruthy();
    const raw = JSON.parse(rawFile!.text) as {
      points_raw_text: string;
      taken_at_ns: string;
      evidence_integrity: { raw_preserved: string; raw_sha256: string; raw_utf8_bytes: number };
    };
    const analysis = JSON.parse(analysisFile!.text) as {
      sampling: { evidence_integrity: { raw_preserved: string; raw_sha256: string } };
      points_raw_text?: string;
      points: Array<Record<string, unknown>>;
    };
    // 原文未被丢弃：raw 文件里就是原始响应字节对应的文本。
    expect(raw.points_raw_text).toBe(bigText);
    expect(raw.evidence_integrity.raw_preserved).toBe("SEPARATE_FILE");
    expect(raw.taken_at_ns).toMatch(/^\d+$/);
    // 两文件用同一 sha256 配对校验。
    expect(analysis.sampling.evidence_integrity.raw_sha256).toBe(
      raw.evidence_integrity.raw_sha256,
    );
    expect(analysis.sampling.evidence_integrity.raw_preserved).toBe("SEPARATE_FILE");
    // 分析文件不含原文（条件必填：SEPARATE_FILE 时在 -raw 文件里）。
    expect(analysis.points_raw_text).toBeUndefined();
    expect(analysis.points).toEqual((JSON.parse(bigText) as { points: unknown[] }).points);
    // 文件名基址一致（仅后缀不同）。
    const base = rawFile!.name.replace(/-raw\.json$/, "");
    expect(analysisFile!.name).toBe(`${base}-analysis.json`);
  });

  it("export_threshold_uses_utf8_bytes", async () => {
    // 多字节字符：按 TextEncoder 字节数判定，而不是 String.length（UTF-16 码元）。
    const rawText = JSON.stringify({
      points: [
        {
          endpoint_id: "lx-nck",
          point_id: 1,
          quality: "GOOD",
          display_name: "主轴转速·第一通道",
          value: "中文值",
          timestamp_ns: 1792460055540482200,
          value_origin: "CURRENT",
        },
      ],
      count: 1,
    });
    const byteLen = new TextEncoder().encode(rawText).length;
    expect(byteLen).toBeGreaterThan(rawText.length);
    const { fetchImpl } = makeFetch({ latestText: rawText });
    const { document: doc } = await buildPointsExport({
      now: () => 1_792_460_055_541,
      fetchImpl,
    });
    expect(doc.sampling.evidence_integrity.raw_utf8_bytes).toBe(byteLen);
    expect(doc.sampling.evidence_integrity.raw_utf8_bytes).not.toBe(rawText.length);
    expect(doc.sampling.evidence_integrity.raw_preserved).toBe("INLINE");
  });

  it("export_timestamp_is_bigint_string", async () => {
    // 毫秒对齐、末 6 位恒为 0；且**不是** `Date.now()*1e6` 的浮点结果。
    const t0 = 1_792_460_055_541;
    const t1 = 1_792_460_055_542;
    // 该毫秒值 * 1e6 在 double 上不可精确表示：BigInt 结果与浮点结果不同。
    // （String(double) 会给最短往返十进制，看起来相同，所以必须比 BigInt 值。）
    expect(BigInt(t0 * 1e6)).not.toBe(BigInt(t0) * 1_000_000n);
    let call = 0;
    const { fetchImpl } = makeFetch({});
    const { document: doc } = await buildPointsExport({
      now: () => (call++ === 0 ? t0 : t1),
      fetchImpl,
    });
    expect(doc.taken_at_ns).toBe(String(BigInt(t0) * 1_000_000n));
    expect(doc.sampling.points_read_at_ns).toBe(String(BigInt(t1) * 1_000_000n));
    expect(doc.taken_at_ns).toMatch(/^\d+$/);
    expect(doc.taken_at_ns.endsWith("000000")).toBe(true);
    expect(doc.sampling.points_read_at_ns.endsWith("000000")).toBe(true);
    // 浮点乘法的精确值（BigInt(double)）与 BigInt 计算不同 ⇒ 确实走了 BigInt 路径。
    expect(BigInt(doc.taken_at_ns)).not.toBe(BigInt(t0 * 1e6));
    expect(BigInt(doc.taken_at_ns) / 1_000_000n).toBe(BigInt(t0));
  });
});
