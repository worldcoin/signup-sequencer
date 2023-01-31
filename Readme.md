# Worldcoin Sign-up Sequencer

![lines of code](https://img.shields.io/tokei/lines/github/worldcoin/signup-sequencer)
[![dependency status](https://deps.rs/repo/github/worldcoin/signup-sequencer/status.svg)](https://deps.rs/repo/github/worldcoin/signup-sequencer)
[![codecov](https://codecov.io/gh/worldcoin/signup-sequencer/branch/main/graph/badge.svg?token=WBPZ9U4TTO)](https://codecov.io/gh/worldcoin/signup-sequencer)
[![CI](https://github.com/worldcoin/signup-sequencer/actions/workflows/build-test-deploy.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/build-test-deploy.yml)

## API


## Database

```shell
docker run --rm -ti -p 5432:5432 -e POSTGRES_PASSWORD=password postgres
```

## Hints

Lint, build, test, run

```shell
cargo fmt && cargo clippy --all-targets --features "bench, mimalloc" && cargo build --all-targets --features "bench, mimalloc" && cargo test --all-targets --features "bench, mimalloc" && cargo run --
```

Run benchmarks

```shell
cargo criterion
```
