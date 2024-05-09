![lines of code](https://img.shields.io/tokei/lines/github/worldcoin/signup-sequencer)
[![dependency status](https://deps.rs/repo/github/worldcoin/signup-sequencer/status.svg)](https://deps.rs/repo/github/worldcoin/signup-sequencer)
[![CI](https://github.com/worldcoin/signup-sequencer/actions/workflows/test.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/test.yml)
[![Audit](https://github.com/worldcoin/signup-sequencer/actions/workflows/audit.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/audit.yml)

# Worldcoin Sign-up Sequencer

Sign-up Sequencer does sequencing of data (identities) that are committed in a batch to an Ethereum Smart Contract.

## Table of Contents

1. [Introduction](#introduction)
2. [Getting Started](#getting-started)
3. [Tests](#tests)
4. [Contributing](#contributing)

## Introduction

Sequencer has 6 API routes.

1. `/insertIdentity` - Accepts identity commitment hash as input which gets added in queue for processing.
   Identities go through three tasks.
    1. Insertion: In the initial stage, the identities are placed into the Sequencer's database.
       The database is polled every few seconds and added to insertion task.
    2. Processing: The processing of identities, where current batching tree is taken and processed so we
       end up with pre root (the root of tree before proofs are generated), post root, start index and
       identity commitments (with their proofs). All of those get sent to a [prover](#semaphore-mtb) for proof
       generation.
       The identities transaction is then mined, with aforementioned fields and pending identities are sent to task to
       be mined on-chain.
    3. Mining: The transaction ID from processing task gets mined and Sequencer database gets updated accordingly.
       Now with blockchain and database being in sync, the mined tree gets updated as well.
2. `/inclusionProof` - Takes the identity commitment hash, and checks for any errors that might have occurred in the
   insert identity steps.
   Then leaf index is fetched from the database, corresponding to the identity hash provided, and then we check if the
   identity is
   indeed in the tree. The inclusion proof is then returned to the API caller.
3. `/deleteIdentity` - Takes an identity commitment hash, ensures that it exists and hasn't been deleted yet. This
   identity is then scheduled for deletion.
4. `/recoverIdentity` - Takes two identity commitment hashes. The first must exist and will be scheduled for deletion
   and the other will be inserted as a replacement after the first identity has been deleted and a set amount of time (
   depends on configuration parameters) has passed.
5. `/verifySemaphoreProof` - This call takes root, signal hash, nullifier hash, external nullifier hash and a proof.
   The proving key is fetched based on the depth index, and verification key as well.
   The list of prime fields is created based on request input mentioned before, and then we proceed to verify the proof.
   Sequencer uses groth16 zk-SNARK implementation.
   The API call returns the proof as a response.
6. `/addBatchSize` - Adds a prover with specific batch size to a list of provers.
7. `/removeBatchSize` - Removes the prover based on batch size.
8. `/listBatchSizes` - Lists all provers that are added to the Sequencer.

## Getting Started

### (Local development)

Install pre-requisites on the dev machine

| Os            | Command                                                |
|---------------|--------------------------------------------------------|
| MacOs         | `brew install protobuf pkg-config`                     |
| Ubuntu/Debian | `sudo apt-get install -y protobuf-compiler pkg-config` |

Install [Docker](https://docs.docker.com/get-docker/) - Docker is used to setup the database for testing

Fetch the [postgres](https://hub.docker.com/_/postgres) docker image before running tests.

```shell
docker pull postgres
```

### Local Node

You'll need to run a local node like geth or [ganache](https://archive.trufflesuite.com/ganache/). Start up a new chain
and take note of the dev addresses. You can follow instructions [here](https://book.getfoundry.sh/anvil/).

### Worldcoin id contracts

Worldcoin id contracts are ethereum smart contracts that are used by the sequencer

Clone [contracts-deployer](https://github.com/worldcoin/contract-deployer.git) and follow the steps in the readme there.

### Semaphore-mtb

Semaphore-mtb is a service for batch processing of Merkle tree updates.

Clone [semaphore-mtb](https://github.com/worldcoin/semaphore-mtb) and execute `go build .` (you will need a golang
compiler)

Go build will create an executable named gnark-mbu. If you went through the contracts-deployer,
you will have a generated a keys file that is used by semaphore-mtb. If your deployment contains more than one prover,
then you must run this command for each one and configure them to listen on different ports.

```shell
./gnark-mbu start --keys-file path/to/contracts-deployer/<DEPLOYMENT_NAME>/.cache/keys/<KEY_NAME> --mode <insertion/deletion>
```

### Database

```shell
docker run --rm -ti -p 5432:5432 -e POSTGRES_PASSWORD=password postgres
```

### TX sitter

TX sitter is a service providing API for signup-sequencer to submit transactions on blockchain.

Clone [tx-sitter-monolith](https://github.com/worldcoin/tx-sitter-monolith) and follow build instructions

### Signup-sequencer

Now you need to create a `config.toml` file for signup-sequencer:

```toml
[app]
provers_urls = '[]'

[tree]

[network]
# Address of WorldIDIdentityManager contract on blockchain.
# If you are using anvil the default address should work.
identity_manager_address = "0x48483748eb0446A16cAE79141D0688e3F624Cb73"

[providers]
# Blockchain API URL (anvil or geth)
primary_network_provider = "http://localhost:8545"

[relayer]
kind = "tx_sitter"
# URL of TX-sitter API + API token
tx_sitter_url = "http://localhost:3000/1/api/G5CKNF3BTS2hRl60bpdYMNPqXvXsP-QZd2lrtmgctsnllwU9D3Z4D8gOt04M0QNH"
tx_sitter_address = "0x1d7ffed610cc4cdC097ecDc835Ae5FEE93C9e3Da"
tx_sitter_gas_limit = 2000000

[database]
database = "postgres://postgres:password@localhost:5432/sequencer?sslmode=disable"

[server]
# Port to run signup-sequencer API on
address = "0.0.0.0:8080"
```

The daemon will try to create temporary files in `/data`. If your machine does not have it you could create it:

```shell
mkdir signup_sequencer_data
sudo ln -sf `pwd`/signup_sequencer_data /data
```

And then run the daemon:

```shell
cargo run config.toml
```

### Docker compose

Docker compose from E2E tests can also be used for local development. To run it first export alchemy API key
for anvil fork to work:

```shell
export ALCHEMY_API_KEY=<api-key>
```

Then you can run docker compose (without signup sequencer):

```shell
cd e2e_tests/docker-compose
docker compose up
```

## Tests

Lint, build, test

First ensure you have the docker daemon up and running, then:

```shell
cargo fmt && cargo clippy --all-targets && cargo build --all-targets && cargo test --all-targets
```

## E2E Tests

Before running please make sure to build signup-sequencer image.

```shell
docker build -t signup-sequencer .
```

Then run tests. You need alchemy API key to run docker compose which is used by E2E tests.

```shell
export ALCHEMY_API_KEY=<api-key>
cd e2e_tests/scenarios && cargo test
```

## Contributing

We welcome your pull requests! But also consider the following:

1. Fork this repo from `main` branch.
2. If you added code that should be tested, please add tests.
3. If you changed the API routes, please update this readme in your PR.
4. Ensure that CI tests suite passes (lints as well).
5. If you added dependencies, make sure you add exemptions for `cargo vet`

When you submit code changes, your submissions are understood to be under the same MIT License that covers the project.
Feel free to contact the maintainers if that's a concern.

Report bugs using github issues.
