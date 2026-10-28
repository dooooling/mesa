/**
 * Generic paginated catalog contract.
 *
 * This module deliberately knows nothing about a vendor, protocol, endpoint, or
 * definition schema. An adapter supplies the page loader and the fields that
 * identify a page, its entries, and the opaque continuation cursor.
 *
 * Contract invariants:
 * - cursors are opaque and passed back byte-for-byte as strings;
 * - total is the filtered count before pagination and must stay stable;
 * - duplicate entry identities, cursor cycles, incomplete totals, and page-limit
 * exhaustion fail closed rather than returning a partial catalog.
 */

export interface CatalogPageShape<TEntry> {
  total: number;
  nextCursor: string | null;
  entries: readonly TEntry[];
}

export interface CompleteCatalogResult<TPage, TEntry> {
  firstPage: TPage;
  entries: TEntry[];
  pages: number;
}

export interface ReadCompleteCatalogOptions<TPage, TEntry> {
  loadPage: (cursor: string | undefined) => Promise<TPage>;
  getPage: (page: TPage) => CatalogPageShape<TEntry>;
  getEntryKey: (entry: TEntry) => string;
  maxPages?: number;
}

export type CatalogContractErrorCode =
  | "CATALOG_TOO_MANY_PAGES"
  | "CATALOG_TOTAL_CHANGED"
  | "CATALOG_DUPLICATE_ENTRY"
  | "CATALOG_CURSOR_CYCLE"
  | "CATALOG_INCOMPLETE"
  | "CATALOG_PAGE_INVALID";

export class CatalogContractError extends Error {
  readonly code: CatalogContractErrorCode;

  constructor(code: CatalogContractErrorCode, message: string) {
    super(message);
    this.name = "CatalogContractError";
    this.code = code;
  }
}

/**
 * Read a full catalog using a driver-specific page adapter.
 * The returned page and entries retain the adapter's original values and order.
 */
export async function readCompleteCatalog<TPage, TEntry>(
  options: ReadCompleteCatalogOptions<TPage, TEntry>,
): Promise<CompleteCatalogResult<TPage, TEntry>> {
  const maxPages = options.maxPages ?? 100;
  if (!Number.isInteger(maxPages) || maxPages < 1) {
    throw new CatalogContractError(
      "CATALOG_PAGE_INVALID",
      "maxPages must be a positive integer",
    );
  }

  const entries: TEntry[] = [];
  const seenEntries = new Set<string>();
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  let firstPage: TPage | undefined;
  let expectedTotal: number | undefined;
  let pages = 0;

  for (;;) {
    if (pages >= maxPages) {
      throw new CatalogContractError(
        "CATALOG_TOO_MANY_PAGES",
        `Catalog pagination exceeded ${maxPages} pages; refusing to continue`,
      );
    }

    const page = await options.loadPage(cursor);
    const shape = options.getPage(page);
    if (
      !Number.isInteger(shape.total) ||
      shape.total < 0 ||
      !Array.isArray(shape.entries) ||
      (shape.nextCursor !== null && typeof shape.nextCursor !== "string")
    ) {
      throw new CatalogContractError(
        "CATALOG_PAGE_INVALID",
        "Catalog page has an invalid total, entries list, or continuation cursor",
      );
    }

    pages += 1;
    if (firstPage === undefined) {
      firstPage = page;
      expectedTotal = shape.total;
    } else if (shape.total !== expectedTotal) {
      throw new CatalogContractError(
        "CATALOG_TOTAL_CHANGED",
        `Catalog total changed from ${expectedTotal} to ${shape.total} while paging`,
      );
    }

    for (const entry of shape.entries) {
      const key = options.getEntryKey(entry);
      if (typeof key !== "string" || key.length === 0) {
        throw new CatalogContractError(
          "CATALOG_PAGE_INVALID",
          "Catalog entry identity must be a non-empty string",
        );
      }
      if (seenEntries.has(key)) {
        throw new CatalogContractError(
          "CATALOG_DUPLICATE_ENTRY",
          `Duplicate catalog entry identity: ${key}`,
        );
      }
      seenEntries.add(key);
      entries.push(entry);
    }

    if (shape.nextCursor === null) break;
    if (shape.nextCursor === cursor || seenCursors.has(shape.nextCursor)) {
      throw new CatalogContractError(
        "CATALOG_CURSOR_CYCLE",
        "Catalog continuation cursor repeated; refusing an infinite loop",
      );
    }
    seenCursors.add(shape.nextCursor);
    cursor = shape.nextCursor;
  }

  if (firstPage === undefined || expectedTotal === undefined) {
    throw new CatalogContractError("CATALOG_PAGE_INVALID", "Catalog returned no first page");
  }
  if (entries.length !== expectedTotal) {
    throw new CatalogContractError(
      "CATALOG_INCOMPLETE",
      `Catalog is incomplete: expected ${expectedTotal} entries, received ${entries.length}`,
    );
  }

  return { firstPage, entries, pages };
}
