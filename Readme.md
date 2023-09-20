
![lines of code](https://img.shields.io/tokei/lines/github/worldcoin/signup-sequencer)
[![dependency status](https://deps.rs/repo/github/worldcoin/signup-sequencer/status.svg)](https://deps.rs/repo/github/worldcoin/signup-sequencer)
[![CI](https://github.com/worldcoin/signup-sequencer/actions/workflows/test.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/test.yml)
[![Audit](https://github.com/worldcoin/signup-sequencer/actions/workflows/audit.yml/badge.svg)](https://github.com/worldcoin/signup-sequencer/actions/workflows/audit.yml)

# Worldcoin Sign-up Sequencer

Sign-up Sequencer does sequencing of data (identities) that are commited in a batch to an Ethereum Smart Contract.

## Table of Contents
1. [Introduction](#introduction)
2. [Getting Started](#getting-started)
3. [Tests](#tests)
4. [Contributing](#contributing)

## Introduction

Sequencer has 6 API routes.

1. `/insertIdentity` - Accepts identity commitment hash as input which gets added in queue for processing.
    Identities go trough three tasks.
    1. Insertion: In the initial stage, the identities are placed into the Sequencer's database.
    The database is polled every few seconds and added to insertion task.
    2. Processing: The processing of identities, where current batching tree is taken and processed so we we
    end up with pre root (the root of tree before proofs are generated), post root, start index and
    identity commitments (with their proofs). All of those get sent to a [prover](#semaphore-mtb) for proof generation.
    The identities transaction is then mined, with aforementioned fields and pending identities are sent to task to be mined on-chain.
    3. Mining:  The transaction ID from processing task gets mined and Sequencer database gets updated accordingly.
    Now with blockchain and database being in sync, the mined tree gets updated as well.
2. `/inclusionProof` - Takes the identity commitment hash, and checks for any errors that might have occurred in the insert identity steps.
    Then leaf index is fetched from the database, corresponding to the identity hash provided, and then the we check if the identity is
    indeed in the tree. The inclusion proof is then returned to the API caller.
3. `/verifySemaphoreProof` - This call takes root, signal hash, nullifier hash, external nullifier hash and a proof.
    The proving key is fetched based on the depth index, and verification key as well.
    The list of prime fields is created based on request input mentioned before, and then we proceed to verify the proof.
    Sequencer uses groth16 zk-SNARK implementation.
    The API call returns the proof as response.
4.  `/addBatchSize` - Adds a prover with specific batch size to a list of provers.
5.  `/removeBatchSize` - Removes the prover based on batch size.
6.  `/listBatchSizes` - Lists all provers that are added to the Sequencer.



## Getting Started
### (Local development)
Install Protobuf compiler

| Os            | Command                                     |
| ------------- | ------------------------------------------- |
| MacOs         | `brew install protobuf`                     |
| Ubuntu/Debian | `sudo apt-get install -y protobuf-compiler` |

Install [Docker](https://docs.docker.com/get-docker/) - Docker is used to setup the database for testing

Fetch the [postgres](https://hub.docker.com/_/postgres) docker image before running tests.

```shell
docker pull postgres
```

### Worldcoin id contracts
Worldcoin id contracts are ethereum smart contracts that are used by the sequencer

Clone [worldcoin id contracts](https://github.com/worldcoin/world-id-contracts) and execute `make build && npm install`

You will need a local ethereum chain, like geth or ganache.
Start up your local ethereum network and generate an account.
Go to your worldcoin id contracts repo folder and do `make deploy`. Follow the steps provided by the script.
Make sure you use a private key from your local ethereum network account.
!! Make a note of all the addresses generated by the script !!

### Semaphore-mtb
Semaphore-mtb is a service for batch processing of Merkle tree updates.

Clone [semaphore-mtb](https://github.com/worldcoin/semaphore-mtb) and execute `go build .` (you will need a golang compiler)

Go build will create a executable named gnark-mbu.  If you went trough the worldcoin id contracts deploy script,
you will have a generated keys file that is used by semaphore-mtb.
```shell
./gnark-mbu start --keys-file path/to/world-id-contracts/mtb/keys
```

### Database

```shell
docker run --rm -ti -p 5432:5432 -e POSTGRES_PASSWORD=password postgres
```

Now you are ready to start up sequencer service!
```shell
TREE_DEPTH=*your tree depth (eg. 16)* cargo run -- --batch-size *batch size for semaphore-mtb (eg. 3)* --batch-timeout-seconds 10 --database postgres://postgres:password@0.0.0.0:5432 --identity-manager-address *address from worldcoin id contracts identity manager*
--signing-key *private key you used to deploy smart contracts*
```

## Tests

Lint, build, test

```shell
cargo fmt && cargo clippy --all-targets && cargo build --all-targets && cargo test --all-targets
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
