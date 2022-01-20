# Rust project template

![lines of code](https://img.shields.io/tokei/lines/github/recmo/rust-service-template)
[![dependency status](https://deps.rs/repo/github/recmo/rust-service-template/status.svg)](https://deps.rs/repo/github/recmo/rust-service-template)
[![codecov](https://img.shields.io/codecov/c/github/recmo/rust-service-template)](https://codecov.io/gh/Recmo/rust-service-template)
[![Build, Test & Deploy](https://github.com/recmo/rust-service-template/actions/workflows/build-test-deploy.yml/badge.svg)](https://github.com/recmo/rust-service-template/actions/workflows/build-test-deploy.yml)

## Features

### Continuous Integration

* Build, test and deploy
  * Run rustfmt, clippy and doc build


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

## Deployment

Using GitHub actions for each PR it will push a Docker container image to the [Github container registry](ghcr.io). A Helm chart is included for easy deployment to Kubernetes clusters. The ingress rule assumes a Traefik frontend.


Todo: Tutorial using: `https://github.com/redkubes/otomi-core`
## Hints

Lint, build, test, run

```shell
cargo fmt && cargo clippy --all-targets --all-features && cargo build --all-targets --all-features && cargo test && cargo run --
```

Run benchmarks

```shell
cargo bench --bench criterion --features="bench proptest"
```

## How to use the template

Update `Cargo.toml` and regenerate `deploy/Chart.yaml` from it using the included script:

```shell
./deploy/generate.py > ./deploy/Chart.yaml
```

Change the name of the crate from `rust_service_template` to the new name in `./criterion.rs` and `./src/cli/main.rs`.

Implement your service in `src/lib.rs`.

If your service makes outbound connections, add egress rules to `deploy/templates/network-policy.yaml`.

Deploy using Helm on a Kubernetes cluster using Traefik for ingress management.


## To do

Copy from Tokio:
* Add license, contributing, and other changelogs

* Rustdocs with Katex.
* Long running / fuzz mode for proptests.
* [`loom`](https://crates.io/crates/loom) support for concurrency testing, maybe [`simulation`](https://github.com/tokio-rs/simulation).
* Add crates.io publishing


## Build images locally

```shell
for arch in x86_64 aarch64; do
  docker run --rm \
    -v "$(pwd)":/src \
    -v $HOME/.cargo:/usr/local/cargo:Z \
    -v /usr/local/cargo/bin \
    ghcr.io/recmo/rust-static-build:1.58-$arch \
    cargo build --locked --release --features mimalloc
done
for arch in amd64 arm64; do
  docker build --platform linux/$arch --tag rust-service-template:latest-$arch .
done
docker manifest rm ghcr.io/recmo/rust-service-template:latest
docker manifest create \
    rust-service-template:latest \
    rust-service-template:latest-amd64 \
    rust-service-template:latest-arm64
docker manifest inspect rust-service-template:latest
```
