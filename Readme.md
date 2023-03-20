# Worldcoin Sign-up Sequencer

![lines of code](https://img.shields.io/tokei/lines/github/worldcoin/signup-sequencer)
[![dependency status](https://deps.rs/repo/github/worldcoin/signup-sequencer/status.svg)](https://deps.rs/repo/github/worldcoin/signup-sequencer)
[![codecov](https://codecov.io/gh/worldcoin/signup-sequencer/branch/main/graph/badge.svg?token=WBPZ9U4TTO)](https://codecov.io/gh/worldcoin/signup-sequencer)
[![CI](https://github.com/worldcoin/signup-sequencer/actions/workflows/build-test-deploy.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/build-test-deploy.yml)

## Setup
Install Protobuf compiler

| Os            | Command                                     |
| ------------- | ------------------------------------------- |
| MacOs         | `brew install protobuf`                     |
| Ubuntu/Debian | `sudo apt-get install -y protobuf-compiler` |

Install [Docker](https://docs.docker.com/get-docker/) - Docker is used to setup the database for testing

Fetch the [postgres](https://hub.docker.com/_/postgres) docker image before running tests.

```
docker pull postgres
```

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
