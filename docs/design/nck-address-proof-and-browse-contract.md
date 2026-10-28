# NCK address evidence and Browse contract: integrated implementation plan

## Delivery rule

Implement this as one integrated branch / one PR / one acceptance pass. Do not split protocol, driver, frontend, and evidence work into separate PRs. Keep `r1.json` unchanged until each native address mapping is independently supported.

## Workstream A — native NCK address evidence

### Questions to close

- Which authoritative source defines the NCK VarSpec fields `area_unit`, `area_no/unit`, `column`, `line`, `module`, and `line_count` for the target SINUMERIK generation?
- Which fields are protocol encoding facts, which are block-specific NCK semantics, and which require device evidence?
- Is a given `module` value tied to a logical block (S/RP/SSP/SGA/SEGA), a connection/entry path, or another device-specific mapping?

### Implementation

- Add a machine-readable evidence matrix for the five current observed blocks: S=127, RP=21, SSP=114, SGA=115, SEGA=112.
- For each row, record source URL/document and page or source symbol, raw request bytes, decoded fields, test/sample provenance, device/software context if known, confidence class, and unresolved claims.
- Keep three evidence classes separate: protocol-layout evidence, Siemens semantic/manual evidence, and actual-device observation.
- Add a validator that rejects missing evidence provenance, contradictory field claims, and any claim that a manual semantic match proves a native `module` value.
- Do not update any production address or promote any definition solely because the manual entry name/column matches.

### Acceptance

- Each mapping claim is either backed by an authoritative source or explicitly remains unresolved.
- Golden wire-byte tests cover the exact request bytes for each confirmed block.
- `r1.json` blob remains unchanged in this change unless a later, reviewed evidence record justifies a specific edit.

## Workstream B — Browse/Discovery contract gap

### Current gap

The protobuf `BrowseNode` exposes only `id`, `label`, `kind`, `data_type`, `access`, `has_children`, and opaque `binding_json`. The frontend currently needs an injected driver policy to infer whether a node is selectable, what payload it carries, and why it is blocked. That removes vendor literals from the shared component but is not full contract-level decoupling.

### Implementation

- Design an additive, backwards-compatible Browse selection contract in `proto/driver.proto` that can express: selection mode (generic resource / semantic payload / not selectable), an opaque selection payload, and structured diagnostic reasons.
- Keep the Core transport layer semantically neutral: pass the fields through without interpreting driver-specific payloads.
- Update SDK / protocol minor version and in-tree driver manifests together; old drivers with unspecified new fields must continue to work for ordinary resource browsing.
- Move eligibility decisions to the driver/catalog owner. The frontend renders status and diagnostics, dispatches opaque payloads, and never guesses from a vendor string.
- Preserve the current fail-closed rule: missing/unknown semantic eligibility cannot produce a selectable semantic point.
- Keep `NckSemanticPicker` as the NCK-specific form; do not force a universal form onto all driver types.

### Acceptance

- Non-NCK generic Browse regression tests remain green.
- NCK semantic Browse uses authoritative driver-provided selection state and payload.
- Unknown status, malformed payload, missing handler, or unsupported protocol capability is visibly blocked rather than silently falling back.
- Protocol compatibility tests cover new Core + old driver and old client behavior where supported by the existing version-negotiation contract.

## Workstream C — source reconciliation and readiness

- Keep the 03/2025 NC Variables List Manual results separate from native address proof. `channel.program.name.main` remains `pending_review` until an official source identifies the expected SPARPP/column mapping.
- Track string lengths (66-byte `actBlock`, 32-byte `progName`) as unresolved until an authoritative source or independent device evidence confirms them.
- Track the 11/2023 SINAMICS S120/S150 manual as a separate source gap; 07/2023 is not a substitute.
- Resolve the `axis.encoder.module_number` evidence-chain defect by either exercising the constant in the actual probe path and preserving output, or removing the unsupported measurement claim.
- Preserve the module-26 vs module-130 identity distinction; no fixed-offset conversion or inferred equivalence.
- Do not run live hardware probes as part of CI or without separate explicit authorization.

## Scope boundaries

- No production definition/address changes without an evidence-backed row-by-row review.
- No database migration or `mesa.db` change.
- No hardware test is claimed unless the test was actually run against the real 828D.
- PDF files remain local-only and are not committed to Git.
- Any unresolved item remains visible in the audit output; unresolved does not mean failed software implementation.

## One-PR acceptance gate

- [ ] Evidence matrix and validator pass, including negative tests.
- [ ] Browse protobuf/API/SDK/driver/frontend changes are additive and compatibility-tested.
- [ ] Generic Browse contains no vendor-specific point-kind decisions.
- [ ] Semantic selection is fail-closed and diagnostics are user-visible.
- [ ] Existing production definition blob is unchanged unless individually justified.
- [ ] Full required CI is green on the final HEAD.
- [ ] Final PR description matches actual files and acceptance results.
