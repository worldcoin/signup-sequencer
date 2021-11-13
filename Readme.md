# Rust project template

![lines of code](https://img.shields.io/tokei/lines/github/recmo/rust-app-template)
[![dependency status](https://deps.rs/repo/github/recmo/rust-app-template/status.svg)](https://deps.rs/repo/github/recmo/rust-app-template)
[![codecov](https://img.shields.io/codecov/c/github/recmo/rust-app-template)](https://codecov.io/gh/Recmo/rust-app-template)
[![ci](https://img.shields.io/github/workflow/status/recmo/rust-app-template/ci)](https://github.com/Recmo/rust-app-template/actions?query=workflow%ci)

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
