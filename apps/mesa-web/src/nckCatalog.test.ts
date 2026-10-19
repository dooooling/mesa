// PR-C 第二批 B 门禁 1/5/6/11（门禁 1 的重点）：I-2 目录客户端的契约级测试。
//
// 这里**不测 UI**：分页、游标逐字回传、错误映射与「目录不做 scope 过滤」
// 都是传输层事实，必须在纯函数层钉死，否则一旦页边界漏项，UI 测试只会
// 表现为「少了一个选项」，很难定位。
import { describe, expect, it, vi } from "vitest";
import {
  NckRequestError,
  classifyNckError,
  fetchNckCatalog,
  fetchNckCatalogPage,
  isNckRequestError,
  previewNckPoint,
  type NckCatalogDefinition,
  type NckDefinitionsResponse,
  type NckFetchLike,
} from "./nckCatalog";
import { collectableDefinitions } from "./nckCollectability";

function def(
  variableId: string,
  overrides: Partial<NckCatalogDefinition> = {},
): NckCatalogDefinition {
  const revision = overrides.revision ?? 1;
  return {
    variable_id: variableId,
    revision,
    family: "828d",
    scope: "collect",
    semantics_status: "verified",
    label: { zh: variableId, en: variableId },
    source: [{ kind: "manual", reference: "test" }],
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    identity_template: `${variableId}@channel={channel}`,
    identity_keys: [{ path: "channel", required: true }],
    golden_ref: [{ area: "C", block: "S", variable: "x", entry: "nc_variable" }],
    digest: `digest-${variableId}-${revision}`,
    ...overrides,
  };
}

function envelope(overrides: Partial<NckDefinitionsResponse> = {}): NckDefinitionsResponse {
  return {
    format_version: 1,
    family: "828d",
    revision: 1,
    driver_id: "sinumerik-nck",
    driver_version: "0.1.0",
    note: null,
    total: 0,
    limit: 100,
    next_cursor: null,
    definitions: [],
    ...overrides,
  };
}

function jsonOk(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

describe("门禁 1：目录分页完整读取（不遗漏、不重复）", () => {
  it("catalog_pagination_reads_all_pages_without_loss_or_duplication", async () => {
    const calls: string[] = [];
    const pages: Record<string, NckDefinitionsResponse> = {
      "": envelope({
        total: 5,
        limit: 2,
        next_cursor: "v1-ax.b-1",
        definitions: [def("ax.a"), def("ax.b")],
      }),
      "v1-ax.b-1": envelope({
        total: 5,
        limit: 2,
        next_cursor: "v1-ch.c-1",
        definitions: [def("ax.c"), def("ch.c")],
      }),
      "v1-ch.c-1": envelope({
        total: 5,
        limit: 2,
        next_cursor: null,
        definitions: [def("ch.d")],
      }),
    };
    const fetchImpl: NckFetchLike = async (url) => {
      calls.push(url);
      const parsed = new URL(url, "http://localhost");
      const cursor = parsed.searchParams.get("cursor") ?? "";
      const page = pages[cursor];
      if (!page) throw new Error(`unexpected cursor: ${cursor}`);
      return jsonOk(page);
    };
    const catalog = await fetchNckCatalog({
      driverId: "sinumerik-nck",
      limit: 2,
      fetchImpl,
    });
    expect(catalog.total).toBe(5);
    expect(catalog.pages).toBe(3);
    // 顺序、内容均逐字保留；不重不漏。
    expect(catalog.definitions.map((d) => d.variable_id)).toEqual([
      "ax.a",
      "ax.b",
      "ax.c",
      "ch.c",
      "ch.d",
    ]);
    expect(new Set(catalog.definitions.map((d) => `${d.variable_id}@${d.revision}`)).size).toBe(5);
    // next_cursor 逐字回传（不是重新构造/解析后再拼）。
    expect(calls).toHaveLength(3);
    expect(calls[0]).not.toContain("cursor=");
    expect(calls[1]).toContain("limit=2");
    expect(calls[1]).toContain("cursor=v1-ax.b-1");
    expect(calls[2]).toContain("cursor=v1-ch.c-1");
  });

  it("catalog_duplicate_entry_fails_closed_instead_of_silent_dedupe", async () => {
    const pages: Record<string, NckDefinitionsResponse> = {
      "": envelope({
        total: 3,
        next_cursor: "c1",
        definitions: [def("dup.x"), def("dup.y")],
      }),
      c1: envelope({ total: 3, next_cursor: null, definitions: [def("dup.x")] }),
    };
    const fetchImpl: NckFetchLike = async (url) => {
      const cursor = new URL(url, "http://localhost").searchParams.get("cursor") ?? "";
      return jsonOk(pages[cursor]);
    };
    const error = await fetchNckCatalog({ driverId: "sinumerik-nck", fetchImpl }).catch(
      (e: unknown) => e,
    );
    expect(isNckRequestError(error)).toBe(true);
    expect((error as NckRequestError).code).toBe("CATALOG_DUPLICATE_ENTRY");
  });

  it("catalog_incomplete_and_total_drift_are_rejected", async () => {
    const incomplete: NckFetchLike = async () =>
      jsonOk(envelope({ total: 3, next_cursor: null, definitions: [def("only.one")] }));
    const e1 = await fetchNckCatalog({ driverId: "sinumerik-nck", fetchImpl: incomplete }).catch(
      (e: unknown) => e,
    );
    expect((e1 as NckRequestError).code).toBe("CATALOG_INCOMPLETE");

    const driftPages: Record<string, NckDefinitionsResponse> = {
      "": envelope({ total: 2, next_cursor: "c1", definitions: [def("a.1")] }),
      c1: envelope({ total: 3, next_cursor: null, definitions: [def("a.2")] }),
    };
    const drift: NckFetchLike = async (url) => {
      const cursor = new URL(url, "http://localhost").searchParams.get("cursor") ?? "";
      return jsonOk(driftPages[cursor]);
    };
    const e2 = await fetchNckCatalog({ driverId: "sinumerik-nck", fetchImpl: drift }).catch(
      (e: unknown) => e,
    );
    expect((e2 as NckRequestError).code).toBe("CATALOG_TOTAL_CHANGED");
  });

  it("catalog_does_not_filter_scope_at_transport_layer", async () => {
    // 目录是「目录」：36 collect + 6 identity 必须全部到达；过滤只发生在 UI 模型层。
    const all = [
      ...Array.from({ length: 36 }, (_, i) => def(`syn.collect.${i + 1}`)),
      ...Array.from({ length: 6 }, (_, i) => def(`syn.identity.${i + 1}`, { scope: "identity" })),
    ];
    const fetchImpl: NckFetchLike = async () =>
      jsonOk(envelope({ total: all.length, next_cursor: null, definitions: all }));
    const catalog = await fetchNckCatalog({ driverId: "sinumerik-nck", fetchImpl });
    expect(catalog.definitions).toHaveLength(42);
    const collectable = collectableDefinitions(catalog.definitions);
    expect(collectable).toHaveLength(36);
    expect(collectable.some((d) => d.scope === "identity")).toBe(false);
  });
});

describe("门禁 5/6：错误信封按 kind 分类（不按 code 反推状态）", () => {
  it("http_errors_are_classified_by_error_kind", async () => {
    const cases: Array<{ status: number; kind: string; code: string; expected: string }> = [
      { status: 400, kind: "ConfigurationError", code: "INVALID_QUERY", expected: "request" },
      { status: 404, kind: "AddressError", code: "UNKNOWN_VARIABLE", expected: "not_found" },
      { status: 501, kind: "UnsupportedError", code: "UNSUPPORTED_FAMILY", expected: "unsupported" },
      { status: 503, kind: "InternalError", code: "DEFINITION_DIGEST_MISMATCH", expected: "unavailable" },
      { status: 504, kind: "TimeoutError", code: "TIMEOUT", expected: "timeout" },
      {
        status: 413,
        kind: "ConfigurationError",
        code: "REQUEST_TOO_LARGE",
        expected: "too_large",
      },
    ];
    for (const c of cases) {
      const fetchImpl: NckFetchLike = async () => ({
        ok: false,
        status: c.status,
        json: async () => ({
          error: { code: c.code, kind: c.kind, message: `m-${c.code}` },
        }),
      });
      const error = await fetchNckCatalogPage({ driverId: "sinumerik-nck" }, fetchImpl).catch(
        (e: unknown) => e,
      );
      expect(isNckRequestError(error)).toBe(true);
      const err = error as NckRequestError;
      expect(err.status).toBe(c.status);
      expect(err.code).toBe(c.code);
      expect(err.kind).toBe(c.kind);
      expect(classifyNckError(err.status, err.kind)).toBe(c.expected);
    }

    const networkError = await fetchNckCatalogPage({ driverId: "sinumerik-nck" }, async () => {
      throw new Error("offline");
    }).catch((e: unknown) => e);
    expect((networkError as NckRequestError).status).toBe(0);
    expect(classifyNckError(0, "")).toBe("network");
  });

  it("malformed_error_envelope_is_not_guessed_as_a_domain_error", async () => {
    const fetchImpl: NckFetchLike = async () => ({
      ok: false,
      status: 500,
      json: async () => ({ nope: true }),
    });
    const error = await fetchNckCatalogPage({ driverId: "sinumerik-nck" }, fetchImpl).catch(
      (e: unknown) => e,
    );
    expect((error as NckRequestError).code).toBe("HTTP_500");
    expect(classifyNckError(500, "")).toBe("contract");
  });
});

describe("门禁 3/4/5：预览请求形状与响应校验", () => {
  it("preview_request_has_exactly_three_keys_and_returns_six_fields", async () => {
    const seen: Array<{ url: string; body: Record<string, unknown> }> = [];
    const fetchImpl: NckFetchLike = async (url, init) => {
      seen.push({ url, body: JSON.parse(String(init?.body)) as Record<string, unknown> });
      return jsonOk({
        point_key: "channel.program.status@channel=1",
        identity_mode: "derived",
        scope: "collect",
        variable_id: "channel.program.status",
        definition_revision: 1,
        definition_digest: "d",
      });
    };
    const response = await previewNckPoint(
      {
        driverId: "sinumerik-nck",
        family: "828d",
        kind: "nck_variable",
        parameters: {
          variable_id: "channel.program.status",
          definition_revision: 1,
          target: { channel: 1 },
          arguments: {},
        },
      },
      fetchImpl,
    );
    expect(response.point_key).toBe("channel.program.status@channel=1");
    expect(Object.keys(seen[0].body).sort()).toEqual(["family", "kind", "parameters"]);
    // point_key 不得作为请求字段（它正是要被算出来的东西）。
    expect(seen[0].body).not.toHaveProperty("point_key");
  });

  it("preview_response_missing_point_key_is_a_contract_error", async () => {
    const fetchImpl: NckFetchLike = async () =>
      jsonOk({
        identity_mode: "derived",
        scope: "collect",
        variable_id: "v",
        definition_revision: 1,
        definition_digest: "d",
      });
    const error = await previewNckPoint(
      { driverId: "sinumerik-nck", family: "828d", kind: "nck_variable", parameters: {} },
      fetchImpl,
    ).catch((e: unknown) => e);
    expect((error as NckRequestError).code).toBe("CATALOG_RESPONSE_INVALID");
  });
});
