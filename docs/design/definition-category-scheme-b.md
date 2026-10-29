# Scheme B — authoritative definition categories and evidence correction

## Delivery

One integration branch / one PR / one acceptance pass. Base: `b3005f0b6dc4b6fbc45af2881b1568df7b230439` (PR #152 merge). Do not split definition schema, catalog API, frontend, digest lock, or evidence correction into separate PRs.

## Data/versioning decision

- Keep `r1.json` byte-for-byte immutable. It represents the published revision 1 records and remains available for existing saved selections.
- Publish `r2.json` with all 42 definitions at revision 2 and a formal `category` field. Each category must be one of `cnc_variable`, `system_variable`, `machine_data`, `drive_parameter`, or `unclassified`.
- Add `category` as an optional, omitted-when-absent field in the Rust schema so revision 1 deserializes and retains its canonical digest. Revision 2 must include it in the canonical digest.
- Extend the embedded catalog loader to load multiple revision documents against one lock containing all 84 (id, revision) entries. Preserve exact lookup: a requested revision must never silently fall back to another revision.
- The definitions API may continue exposing published revisions for audit and exact lookups. The user-facing tree must select the latest revision per variable ID for new point creation; editing an existing point must keep its explicitly saved revision.
- `category_basis` is derived, not persisted: `definition` when the selected revision declares category; `navigation_rule` only for legacy revision 1 records that have no category and need backward-compatible display.

## Encoder evidence correction

Revision 2 of `axis.encoder.module_number` must remove the unsupported `measurement` source pointing to `docs/evidence/nck-live-identity-probe.txt` and must not retain `semantics_status=verified` on the strength of that claim. Set the conservative status supported by the remaining identity-probe and frozen-vector evidence, document that native module mapping remains unresolved, and keep the identity-only scope unchanged.

Replace the old positive-assert-known-defect tests with tests that assert the corrected current revision no longer carries the unsupported measurement claim, while revision 1 remains immutable and historical.

## Compatibility and behavior

- Existing saved revision-1 point selections continue to compile and resolve exactly.
- New catalog choices use revision 2, category from the definition, and the existing target/object-probe metadata.
- Search/tree must not show duplicate leaves for the same variable ID. Existing edit flows use the saved revision; new selections use latest published revision.
- No address, decode, module, column, line, length, or wire behavior changes are justified by adding category. Do not change them as part of this schema migration.
- No live hardware probes; no production database changes.

## Tests and acceptance

- Unit tests: optional category backward compatibility; required valid category for revision 2; category enum rejects unknown values; revision monotonicity and no duplicates; digest lock exact 1:1 across r1+r2; exact lookup for rev1 and rev2.
- API tests: catalog reports categories and basis correctly by revision; pagination has no omissions with duplicate variable IDs across revisions; stable total and cursor semantics.
- Frontend tests: new selection chooses latest revision once per variable ID; editing existing revision 1 stays pinned to revision 1; no duplicate tree keys/leaves; category comes only from API.
- Encoder tests: current revision has no unsupported measurement source and is no longer verified on that evidence; historical revision 1 remains present and unchanged.
- Required full CI, front-end tests/build, Rust canonical/quality/platform/e2e checks pass on final HEAD.

## Explicit non-goals

- Do not resolve the native NCK module/area_no mapping by inference.
- Do not mark `channel.program.name.main`, 66/32 string lengths, or the 11/2023 manual gap as solved.
- Do not claim real-device validation.
