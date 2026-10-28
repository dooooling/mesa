# Unified Catalog / Browse / Discovery and 828D source reconciliation

## Goal

Deliver one integrated change for the generic driver catalog/browse/discovery contract and the 828D definition source update. This document is the shared acceptance boundary for the single integration PR; implementation is not split into PR-1 / PR-2.

## Contract boundaries

- Driver owns the semantics and definition data. Core transports an opaque catalog response and maps errors only by `ErrorKind`.
- Browse remains a resource-discovery mechanism, not an implicit allow-list for acquisition.
- A driver may not implement a catalog; the SDK's explicit `DEFINITIONS_UNSUPPORTED` response remains a supported capability outcome.
- Catalog page cursors are opaque. Clients pass them back unchanged.
- The client must reject total drift, duplicate identities, cursor cycles, malformed pages, page-budget exhaustion, and incomplete totals. It must never silently return a partial catalog.
- Semantic eligibility is a separate decision from browse visibility and identity preview. Missing/unknown eligibility metadata fails closed.
- The front end must not encode protocol addresses, module numbers, or vendor-specific variable definitions as UI defaults.

## Implemented in this integration branch

- Added `apps/mesa-web/src/catalogContract.ts`, a vendor-neutral, adapter-based pagination contract.
- Added `apps/mesa-web/src/catalogContract.test.ts` for opaque-cursor round trips, stable totals, duplicate detection, cursor cycles, incomplete catalogs, malformed identities, and page limits.
- Refactored the existing NCK catalog loader to use the shared pagination contract while preserving the existing NCK response parser and public API.
- Recorded the official Siemens 03/2025 NC Variables List Manual URL and online metadata in `docs/reference/sinumerik-828d-manuals.json`. The official online PDF identifies SINUMERIK 828D, CNC software 5.25, 03/2025, document A5E53370586B AC, 1262 pages.

## 828D source reconciliation: safety rule

The official PDF is available online at:

https://cache.industry.siemens.com/dl/files/266/109988266/att_1327302/v1/828D_ncvar_lists_man_0325_en-US.pdf

The web-accessible document confirms the source and exposes searchable page text. It does **not** provide a locally downloaded byte stream in this environment, so this branch does not invent a local SHA-256 or mark the local PDF as present.

The online manual also surfaces potential differences between some existing definition address columns and the manual's documented column indices (for example, `progStatus` is shown at column index 299 in the manual, while the current frozen definition uses column 13; `cmdFeedRateIpo` is shown at column index 166 while the current definition uses column 2). These must not be silently normalized: the manual's OPI column index and the driver's native NCK wire address may not be interchangeable without a protocol-level mapping proof.

Therefore the integrated change must keep these evidence levels separate:

1. Official manual presence / identifier / section / page evidence.
2. OPI manual column index and data type.
3. Mesa native NCK wire address / module / column / line.
4. Real-device read evidence and semantic validation.

A match in (1) or (2) alone does not prove (3) or (4). Until the protocol mapping is independently reconciled, `r1.json` address/decode/status fields remain unchanged; unresolved rows remain explicitly unresolved rather than being promoted to verified.

## Single-PR acceptance checklist

- [ ] Generic pagination contract and adapter tests pass.
- [ ] NCK client behavior remains backward compatible: same public response types, error codes, scope filtering boundary, and opaque cursor behavior.
- [ ] Browse/Discovery remain driver-capability-driven; generic resource browsing for non-NCK drivers does not change.
- [ ] Any semantic UI path obtains authoritative definition metadata and never guesses eligibility or identity.
- [ ] All 13 prior `not_found` rows are reconciled against the 03/2025 manual, with printed page and PDF page tracked separately where they differ.
- [ ] Every proposed native address change is justified by an explicit mapping from the OPI manual representation to the NCK wire address, or remains marked unresolved.
- [ ] The existing production definition blob is not changed without that mapping evidence.
- [ ] Workspace CI and front-end typecheck/tests pass on the final commit.
- [ ] One PR contains the complete integrated change; no intermediate implementation PRs are opened.

## Explicitly not claimed

- No 828D device test was performed by this change.
- A source/manual match is not hardware validation.
- The online PDF metadata is not a local file hash.
- A `candidate` definition is not upgraded merely because a similarly named manual row exists.
