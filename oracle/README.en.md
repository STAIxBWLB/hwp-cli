[한국어](README.md) · [English](README.en.md)

# Independent oracle build audit

`primary-artifacts.lock.json` records only exact official primary artifacts that were independently
downloaded and hash-verified. It is not an image lock and does not make the required certification
oracle available.

No Dockerfile is published yet. The LibreOffice DEB archive depends on an operating-system runtime
closure that is not captured by the archive hash, and no base image digest has been selected. A
reproducible build also needs an actually built image ID/repository digest and an offline runner
attestation. Inventing any of those values would make the oracle result unverifiable.

H2Orestart's OXT does not contain its `COPYING` file. Any future image that redistributes the OXT
must carry the GPL notice and corresponding-source mechanism. An operator-supplied mounted OXT is a
different distribution model but still must be hash-attested by the certification policy.

Until all missing requirements in the lock are resolved, `oracle.mode=required` correctly returns
partial/not-run rather than silently falling back to the native renderer. The structured corpus uses
`oracle.mode=disabled` and makes no independent-office or Hancom parity claim.
