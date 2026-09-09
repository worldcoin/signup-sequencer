# Dependency security checks

CI runs required bans/licenses/sources checks and a separate advisory report.
The policy mirrors world-id-protocol's specific unmaintained-crate exceptions.
There are **no vulnerability advisory exceptions**.

## Security remediation

- Update anyhow, crossbeam-epoch, h2, ring, rustls/webpki and prometheus/protobuf.
- Use AWS-LC for JWT signing/verification instead of RustCrypto RSA.
- Update AWS SDK dependencies and select the modern default HTTPS client.
  Do not enable Cognito's legacy `rustls` feature: it pulls Hyper 0.14/Rustls 0.21.
- Patch archived Ethers' transport dependencies while retaining its API; see
  [vendor/README.md](../vendor/README.md) for provenance and changes.
- Move the local relayer and end-to-end HTTP client to Axum 0.7/Reqwest 0.12.
- Upgrade Arkworks 0.4's logging dependency to tracing-subscriber 0.3.
- Redirect the old Semaphore build-time downloader to maintained Reqwest.
- Replace yaml-rust, adler, fxhash and instant through maintained dependencies.

## Maintenance notices

The five explicit exceptions in deny.toml are already accepted by
world-id-protocol: atomic-polyfill, bincode, derivative, paste and
proc-macro-error2. They are maintenance notices, not accepted vulnerabilities.
Each exception names its dependency path and removal plan.

The stacked Semaphore upgrade removes atomic-polyfill, bincode and
proc-macro-error2, as well as the temporary Arkworks 0.4 patch. The remaining
derivative/paste notices require changes in upstream Arkworks/Alloy/telemetry.
Do not modify persisted formats or proof arithmetic merely to remove a
maintenance notice.

Run `cargo deny check` to check the complete policy. New vulnerabilities fail
the advisory report; they must be remediated rather than added to the baseline.
