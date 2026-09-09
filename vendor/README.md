# Temporary dependency patches

Ethers upstream is archived. These copies preserve its public API while moving
HTTP, WebSocket and JWT processing to patched dependencies. Cargo.toml.orig and
.cargo_vcs_info.json preserve the published manifests and source revisions.
Upstream license texts are included. Versions identify the upstream source,
not a claim that an upstream release contains these local changes.

| Published source | Local changes |
| --- | --- |
| ethers-providers 2.0.14 | Reqwest 0.12, HTTP 1, tokio-tungstenite 0.24, jsonwebtoken 10/AWS-LC, web-time, rustc-hash instead of hashers/fxhash. |
| ethers-middleware 2.0.14 | Reqwest 0.12 and web-time. |
| ethers-etherscan 2.0.14 | Reqwest 0.12. |
| ethers-contract-abigen 2.0.14 | Reqwest 0.12 for its optional online ABI fetching. |
| ark-relations 0.4.0 | tracing-subscriber 0.3.20+ and rename the empty Layer::new_span hook to on_new_span. No proof arithmetic changes. Remove after upgrading Semaphore. |
| reqwest compatibility adapter | New local re-export of Reqwest 0.12.28+, replacing the old Semaphore/SVM downloaders' Reqwest 0.11 implementation. It contains no old Reqwest or Hyper code. Supports only the APIs/features used by these downloaders, not general 0.11 compatibility. |

The Reqwest adapter keeps the old package identity solely to satisfy the
downloaders' version requirements; TLS and HTTP operations are implemented by
the maintained dependency. Its Cargo.lock dependencies make this explicit.

Review source changes by comparing each copy with its corresponding crates.io
release. Replace these patches with maintained upstream dependencies when
practical. Do not remove TLS validation, authentication, or enable old transport
features to resolve compilation failures.
