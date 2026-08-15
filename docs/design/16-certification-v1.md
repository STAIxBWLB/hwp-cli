[한국어](16-certification-v1.ko.md) · [English](16-certification-v1.md)

# Certification v1

`hwp certify INPUT --policy POLICY --report REPORT_DIR` provides a versioned, machine-readable
document certification contract. CLI and MCP (`hwp_certify`) call the same implementation.

## Trust boundary

- The input, policy, fonts, runtime executable, and extension are copied to private immutable
  snapshots before use.
- HWPX ZIP/XML and every HWP5 CFB stream are bounded before semantic parsing. HWP5 limits cover
  stream count/name bytes, per-stream stored and materialized bytes, aggregate materialized bytes,
  and record count/depth. DocInfo, BodyText, Scripts, and compressed BinData reuse one bounded
  decompressed snapshot for repeat imports and feature inspection.
- DefaultJScript macro inspection parses its length-prefixed UTF-16LE blocks and empty sentinel.
  Opaque or malformed script data is `inspection_incomplete`, not evidence that a macro is present
  or absent.
- Layout, page, display-item, image decode, raster, log, and artifact work have separate limits.
  The independent page-count and selected-page render passes must report the same total page count;
  drift is a typed fatal result and publishes no page artifacts.
- Typed render issues use closed code/severity/stage tuples. Details are represented only by
  bounded SHA-256 samples; document text and paths are not retained.
- Font resolution diagnostics are capped at 512 distinct outcomes at the resolver source. The
  513th distinct outcome emits the fatal typed `font_resolution_budget_exceeded` issue and marks
  resolution incomplete, so schema-sized reports cannot silently pass.
- The report directory must not exist and is published with an atomic no-replace rename only
  after its fixed artifact tree and manifest have been audited.
- Page PNGs are written in a private render sub-transaction and merged only after every selected
  page is encoded and validated. A late encode/write failure removes the entire page set.

## Native result scope

`scope=native_only` means the package, repeated semantic import, policy rules, and selected native
render pages passed. `not_detected` diagnostics are algorithm-scoped. Neither native success nor
the optional oracle claims pixel parity with Hancom Office.

## Independent import oracle

The optional/required oracle is intentionally unavailable until an administrator provisions all
trusted environment pins:

- `HWP_CERTIFY_ORACLE_RUNTIME`: Docker-compatible client executable
- `HWP_CERTIFY_ORACLE_EXTENSION`: H2Orestart OXT
- `HWP_CERTIFY_ORACLE_IMAGE`: immutable `repository@sha256:...` reference
- `HWP_CERTIFY_ORACLE_DOCKER_CLIENT_VERSION`
- `HWP_CERTIFY_ORACLE_DOCKER_SERVER_VERSION`
- `HWP_CERTIFY_ORACLE_IMAGE_ID`: observed `sha256:...` image ID

The policy pins the runtime executable, LibreOffice executable, extension, and image digest. The
trusted environment additionally pins the observed Docker client/server and image ID. A daemon
that cannot be observed against those pins yields `host_daemon_unattested`; required mode cannot
pass.

Reports expose only SHA-256 values for the observed client/server versions and full image
reference. The deployment's registry/repository name and trusted environment values are never
published; the content-addressed image ID remains visible as `sha256:...`.

The container runs offline, read-only, capability-free, and resource-limited. `/output` is a
size-bounded tmpfs. Only `/output/oracle-result.json` and `/output/import.pdf` are copied to a
private quarantine, after which container removal is retried and verified by an expected
not-found inspection before any artifact can be published.

No supported public oracle image is shipped by this repository, so a full oracle profile is not
provisioned by default. LibreOffice 26.2.5 and H2Orestart 0.7.12 are reference component versions;
the official H2Orestart v0.7.12 `H2Orestart.oxt` release asset digest is
`7b5f6f247ed9213776f28a86f3c84d50c94e6d99751c20e2d62bb59e59a76566`. The exact Docker
runtime/image and LibreOffice executable hashes remain deployment-specific and must not be
invented.

`oracle/primary-artifacts.lock.json` records the exact official LibreOffice 26.2.5 x86_64 DEB
archive (`2f03bfb2...c1bed1e`), its signature URL, and the H2Orestart release/tag/license evidence.
It deliberately has no image digest or Dockerfile: the base image and LibreOffice runtime
dependency closure are not pinned, and no built image/runner attestation exists. The OXT archive
also omits its GPL `COPYING` file, so future redistribution requires explicit license and
corresponding-source handling. Required oracle mode remains partial until those gaps are closed.

## Optional evidence checks

The document policy may pin two optional, content-free evidence artifacts. Each is read with a
64 KiB bounded read from a path relative to the policy file, parsed against its closed contract,
and fails closed when missing or invalid. Absent sections produce exactly the pre-existing
report shape; a failed section forces `overall=failed` (and an un-run oracle), which the report
schema expresses through dedicated `localPassed + evidenceFailed` branches.

- `document.preservation`: loads a `preservation-report-v1` artifact (for example from
  `hwp convert --loss-report`). The check passes when the aggregated loss total — the sum of
  event counts — is at most `max_loss_codes` (default 0). The report echoes only the aggregated
  per-code counts as `checks.preservation`. Failures use `preservation_loss_detected`;
  missing/invalid artifacts use `preservation_report_invalid`.
- `document.hancom_open`: loads a `hancom-verification-receipt-v1` artifact attesting that a
  Hancom Office application opened the document without repair or damage warnings. With
  `require_pass` (default true) the receipt result must be `pass`. The report echoes only the
  receipt's `application`, `verified_at`, and `verifier` as `checks.hancom_open`. A non-pass
  receipt uses `hancom_open_not_attested`; missing/invalid receipts use
  `hancom_open_receipt_invalid` and echo nothing.

Neither check claims Hancom rendering parity; they attest only the specific external evidence
they name.

## Schemas and consumers

- `schemas/certification-policy-v1.schema.json`
- `schemas/certification-report-v1.schema.json`
- `schemas/certification-oracle-result-v1.schema.json`
- `schemas/preservation-report-v1.schema.json` (optional `preservation` evidence input)
- `schemas/hancom-verification-receipt-v1.schema.json` (optional `hancom_open` evidence input)

Consumers such as Maru must validate the report schema and then verify the runtime invariants that
JSON Schema cannot express: typed issue count/hash recomputation, exact selected-page diagnostics
and page artifacts, unique artifact paths, and `oracle/import.pdf` only for a passed
`native_plus_independent_import` result. Cardinalities are defined as follows: `report.artifacts`
contains at most 257 entries (256 page PNGs plus one oracle PDF); `manifest.files` adds
`report.json` for at most 258; the published tree adds `manifest.json` for an exact cap of 259.
The internal publisher guard of 260 is implementation slack and is not the consumer limit.
