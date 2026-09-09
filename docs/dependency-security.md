# Dependency security checks

CI runs required bans/licenses/sources checks and a separate, nonblocking
advisory report. There are **no vulnerability advisory exceptions**.

## Dependency updates

- Upgrade the Semaphore family to 0.6.0 and use Rust 1.90.
- Update maintained dependencies, including config and prometheus/protobuf.
- Use AWS-LC for application JWT signing/verification instead of RustCrypto RSA.
- Update AWS SDK dependencies and select the modern default HTTPS client.
- Move the local relayer and end-to-end HTTP client to Axum 0.7/Reqwest 0.12.

There are no vendored crates or local compatibility shims. Ethers still brings
legacy transitive dependencies, so these upgrades do not eliminate every
advisory. Remaining findings stay visible rather than being hidden by
vulnerability exceptions.

## Maintenance notices

The derivative and paste exceptions match world-id-protocol's accepted
unmaintained-crate notices. They are not accepted vulnerabilities. The Semaphore
upgrade removes the need for the old atomic-polyfill, bincode and
proc-macro-error2 exceptions.

Run `cargo deny check` to check the complete policy. Vulnerabilities fail the
advisory report and require upstream upgrades or dependency replacement.
