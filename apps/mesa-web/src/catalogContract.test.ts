import { describe, expect, it } from "vitest";
import {
  CatalogContractError,
  readCompleteCatalog,
  type CatalogPageShape,
} from "./catalogContract";

interface TestEntry {
  id: string;
  value: number;
}

interface TestPage {
  total: number;
  next_cursor: string | null;
  entries: TestEntry[];
  provider_note: string;
}

const pageShape = (page: TestPage): CatalogPageShape<TestEntry> => ({
  total: page.total,
  nextCursor: page.next_cursor,
  entries: page.entries,
});

const makePage = (
  entries: TestEntry[],
  overrides: Partial<TestPage> = {},
): TestPage => ({
  total: entries.length,
  next_cursor: null,
  entries,
  provider_note: "kept opaque",
  ...overrides,
});

describe("vendor-neutral catalog pagination contract", () => {
  it("passes opaque cursors through and preserves entry order", async () => {
    const cursors: Array<string | undefined> = [];
    const first = makePage([{ id: "a", value: 1 }], {
      total: 3,
      next_cursor: "opaque:cursor / v2?x=1",
    });
    const second = makePage([{ id: "b", value: 2 }], {
      total: 3,
      next_cursor: "second-cursor",
    });
    const third = makePage([{ id: "c", value: 3 }], { total: 3 });
    const pages = new Map<string, TestPage>([
      ["", first],
      ["opaque:cursor / v2?x=1", second],
      ["second-cursor", third],
    ]);

    const result = await readCompleteCatalog({
      loadPage: async (cursor) => {
        cursors.push(cursor);
        const page = pages.get(cursor ?? "");
        if (!page) throw new Error("unexpected cursor");
        return page;
      },
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
    });

    expect(cursors).toEqual([undefined, "opaque:cursor / v2?x=1", "second-cursor"]);
    expect(result.entries.map((entry) => entry.id)).toEqual(["a", "b", "c"]);
    expect(result.firstPage.provider_note).toBe("kept opaque");
    expect(result.pages).toBe(3);
  });

  it("rejects total drift, duplicates, cursor cycles, and incomplete catalogs", async () => {
    const totalDrift = await readCompleteCatalog({
      loadPage: async (cursor) =>
        cursor
          ? makePage([{ id: "b", value: 2 }], { total: 3 })
          : makePage([{ id: "a", value: 1 }], { total: 2, next_cursor: "c1" }),
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
    }).catch((error: unknown) => error);
    expect(totalDrift).toBeInstanceOf(CatalogContractError);
    expect((totalDrift as CatalogContractError).code).toBe("CATALOG_TOTAL_CHANGED");

    const duplicate = await readCompleteCatalog({
      loadPage: async (cursor) =>
        cursor
          ? makePage([{ id: "a", value: 2 }], { total: 2 })
          : makePage([{ id: "a", value: 1 }], { total: 2, next_cursor: "c1" }),
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
    }).catch((error: unknown) => error);
    expect((duplicate as CatalogContractError).code).toBe("CATALOG_DUPLICATE_ENTRY");

    const cycle = await readCompleteCatalog({
      loadPage: async () => makePage([{ id: "a", value: 1 }], {
        total: 2,
        next_cursor: "loop",
      }),
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
    }).catch((error: unknown) => error);
    expect((cycle as CatalogContractError).code).toBe("CATALOG_CURSOR_CYCLE");

    const incomplete = await readCompleteCatalog({
      loadPage: async () => makePage([{ id: "a", value: 1 }], { total: 2 }),
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
    }).catch((error: unknown) => error);
    expect((incomplete as CatalogContractError).code).toBe("CATALOG_INCOMPLETE");
  });

  it("rejects page-limit exhaustion and malformed entry identities", async () => {
    const tooMany = await readCompleteCatalog({
      loadPage: async () => makePage([{ id: "a", value: 1 }], {
        total: 3,
        next_cursor: "next",
      }),
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
      maxPages: 2,
    }).catch((error: unknown) => error);
    expect((tooMany as CatalogContractError).code).toBe("CATALOG_TOO_MANY_PAGES");

    const invalidKey = await readCompleteCatalog({
      loadPage: async () => makePage([{ id: "", value: 1 }]),
      getPage: pageShape,
      getEntryKey: (entry) => entry.id,
    }).catch((error: unknown) => error);
    expect((invalidKey as CatalogContractError).code).toBe("CATALOG_PAGE_INVALID");
  });
});
