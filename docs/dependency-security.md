# Dependency security checks

CI runs `cargo deny check bans licenses sources` as a required check and
`cargo deny check advisories` as a separate, non-blocking report, matching
world-id-protocol. The advisory ignore list is empty: a passing required check
does not mean the dependency graph is free of security advisories.

## Remediation on the cargo-deny branch

- Update anyhow, crossbeam-epoch, h2 0.4, ring 0.17, and rustls 0.23/webpki to
  patched releases.
- Upgrade prometheus to 0.14 to replace vulnerable protobuf 2 with protobuf 3.7.2.
- Use jsonwebtoken's AWS-LC backend in both production and test utilities,
  removing the vulnerable RustCrypto RSA implementation from the active graph.
- Upgrade config to 0.15 (yaml-rust2) and color-eyre/backtrace (adler2).
- Update the yanked spin 0.9.8 release.

## Remaining work

The legacy dependency graph still reports advisories. Do not suppress these
findings to make the advisory job green.

| Dependency | Required follow-up |
| --- | --- |
| h2 0.3 | Migrate the Hyper 0.14 consumers, including Ethers, AWS Smithy, and the older Axum/Reqwest clients, to a patched HTTP stack. |
| ring 0.16 | Replace Ethers' jsonwebtoken 8 dependency through an upstream update or migration. |
| rustls-webpki 0.101 | Migrate the old Rustls consumers in the Ethers/AWS HTTP and WebSocket stacks. |
| tracing-subscriber 0.2 | Update the Arkworks dependency that still requires this version. |
| Unmaintained crates | Replace remaining transitive atomic-polyfill, bincode, derivative, fxhash, instant, paste, proc-macro-error2, and rustls-pemfile through their parent libraries. |

Re-run the advisory report after the stacked semaphore upgrade; its graph differs
from this branch. Library migrations must retain authentication, TLS validation,
RPC/WebSocket behavior, persisted data compatibility, and proof compatibility.
