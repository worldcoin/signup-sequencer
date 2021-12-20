# Worldcoin Sign-up Sequencer

![lines of code](https://img.shields.io/tokei/lines/github/worldcoin/signup-sequencer)
[![dependency status](https://deps.rs/repo/github/worldcoin/signup-sequencer/status.svg)](https://deps.rs/repo/github/worldcoin/signup-sequencer)
[![codecov](https://codecov.io/gh/worldcoin/signup-sequencer/branch/main/graph/badge.svg?token=WBPZ9U4TTO)](https://codecov.io/gh/worldcoin/signup-sequencer)
[![ci](https://github.com/worldcoin/signup-sequencer/actions/workflows/ci.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/ci.yml)
[![cd](https://github.com/worldcoin/signup-sequencer/actions/workflows/cd.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/cd.yml)

## Hints

Lint, build, test, run

```shell
cargo fmt && cargo clippy --all-targets --all-features && cargo build --all-targets --all-features && cargo test --all-targets --all-features && cargo run --
```

Run benchmarks

```shell
cargo criterion
```


<https://www.dynatrace.com/support/help/extend-dynatrace/opentelemetry/opentelemetry-ingest/opent-rust>


## Telemetry

### Logging

Logs are written to the console. The default log format is `pretty` for local builds and `json` for containers.

### Traces


Start a Jaeger tracing server

```shell
docker run --rm -ti -p6831:6831/udp -p6832:6832/udp -p16686:16686 jaegertracing/all-in-one:latest
```

```shell
open "http://localhost:16686/"
```

Global tracing


### Metrics

