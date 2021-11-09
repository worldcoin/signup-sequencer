# Rust project template

![lines of code](https://img.shields.io/tokei/lines/github/worldcoin/signup-commander-rust)
[![dependency status](https://deps.rs/repo/github/worldcoin/signup-commander-rust/status.svg)](https://deps.rs/repo/github/worldcoin/signup-commander-rust)
[![codecov](https://codecov.io/gh/worldcoin/signup-commander-rust/branch/main/graph/badge.svg?token=WBPZ9U4TTO)](https://codecov.io/gh/worldcoin/signup-commander-rust)
[![ci](https://github.com/worldcoin/signup-commander-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/worldcoin/signup-commander-rust/actions/workflows/ci.yml)
[![cd](https://github.com/worldcoin/signup-commander-rust/actions/workflows/cd.yml/badge.svg)](https://github.com/worldcoin/signup-commander-rust/actions/workflows/cd.yml)

**Main features.** Comes with the kitchen sink. Remove what you don't need.

* Command line argument parsing using `StructOpt`.
* Version info including commit hash.
* Error handling using `anyhow` and `thiserror`.
* Logging using `tracing` with `log` and `futures` compatibility, `-v`, `-vv`, etc. command line arguments.
* Preloaded with `serde`, `rand`, `rayon`, `itertools`.
* Tests using `proptest`, `pretty_assertions` and `float_eq`. I recommend the [closure style](https://docs.rs/proptest/0.10.1/proptest/macro.proptest.html#closure-style-invocation) proptests.
* Benchmarks using `criterion` (run `cargo criterion`).
* Dependencies build optimized, also in dev build.
* From scratch Docker build statically linked to musl.

## Hints

Lint, build, test, run

```shell
cargo fmt && cargo clippy --all-targets --all-features && cargo build --all-targets --all-features && cargo test && cargo run --
```

Run benchmarks

```shell
cargo bench --bench criterion --features="bench proptest"
```
## To do

* Code coverage in CI
* Build cache in CI
* no_std support, CI test using no-std target.
* To do scraper in CI.
* `--threads` cli argument for Rayon worker pool size.
* `--trace` cli argument for `tracing-chrome`.
* `--seed` cli argument for deterministic `rand`.
* Rustdocs with Katex.
* Long running / fuzz mode for proptests.
* [`loom`](https://crates.io/crates/loom) support for concurrency testing, maybe [`simulation`](https://github.com/tokio-rs/simulation).
* Run benchmarks in CI on dedicated machine.
* Generate documentation in CI
* Add code coverage to CI
* Add license, contributing, and other changelogs
* Add ISSUE_TEMPLATE, PR template, etc.
* Add crates.io publishing
* Build ARM binary


```shell
docker build --platform linux/arm64 --progress plain . --build-arg TARGET=aarch64-unknown-linux-musl
```

```shell
docker build --platform linux/amd64 --progress plain . --build-arg TARGET=x86_64-unknown-linux-musl
```
