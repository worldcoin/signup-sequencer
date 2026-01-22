FROM rust:1.86-slim-bookworm AS build-env

WORKDIR /src

# Install dependencies
RUN apt-get update && \
    apt-get install -y git curl build-essential libssl-dev texinfo libcap2-bin pkg-config

# Copy only rust-toolchain.toml for better caching
COPY ./rust-toolchain.toml ./rust-toolchain.toml

# Set environment variables
ENV PATH="/root/.cargo/bin:${PATH}" \
    RUSTUP_HOME="/root/.rustup" \
    CARGO_HOME="/root/.cargo"

# Install the toolchain
RUN rustup component add cargo

# TODO: Hacky but it works
RUN mkdir -p ./src \
    && mkdir -p ./crates/cognitoauth/src \
    && mkdir -p ./crates/micro-oz/src \
    && mkdir -p ./crates/oz-api/src \
    && mkdir -p ./crates/postgres-docker-utils/src \
    && mkdir -p ./crates/test-utils/src \
    && mkdir -p ./crates/tx-sitter-client/src \
    && mkdir -p ./e2e_tests/scenarios/src

# Copy only Cargo.toml for better caching
COPY ./build.rs ./build.rs
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock
COPY ./crates/cognitoauth/Cargo.toml ./crates/cognitoauth/Cargo.toml
COPY ./crates/micro-oz/Cargo.toml ./crates/micro-oz/Cargo.toml
COPY ./crates/oz-api/Cargo.toml ./crates/oz-api/Cargo.toml
COPY ./crates/postgres-docker-utils/Cargo.toml ./crates/postgres-docker-utils/Cargo.toml
COPY ./crates/test-utils/Cargo.toml ./crates/test-utils/Cargo.toml
COPY ./crates/tx-sitter-client/Cargo.toml ./crates/tx-sitter-client/Cargo.toml
COPY ./e2e_tests/scenarios/Cargo.toml ./e2e_tests/scenarios/Cargo.toml

RUN echo "fn main() {}" > ./src/main.rs \
    && echo "fn main() {}" > ./crates/cognitoauth/src/main.rs \
    && echo "fn main() {}" > ./crates/micro-oz/src/main.rs \
    && echo "fn main() {}" > ./crates/oz-api/src/main.rs \
    && echo "fn main() {}" > ./crates/postgres-docker-utils/src/main.rs \
    && echo "fn main() {}" > ./crates/test-utils/src/lib.rs \
    && echo "fn main() {}" > ./crates/tx-sitter-client/src/main.rs \
    && echo "fn main() {}" > ./e2e_tests/scenarios/src/main.rs

# Prebuild dependencies
RUN cargo fetch \ 
    && cargo build --release --workspace

# Copy all the source files
# .dockerignore ignores the target dir
COPY . .

# Build the sequencer
RUN cargo fetch \
    && cargo build --release

# cc variant because we need libgcc and others
FROM gcr.io/distroless/cc-debian12:nonroot

# Expose Prometheus
ENV PROMETHEUS="http://0.0.0.0:9998/metrics"
EXPOSE 9998/tcp
LABEL prometheus.io/scrape="true"
LABEL prometheus.io/port="9998"
LABEL prometheus.io/path="/metrics"

# Copy the sequencer binary
COPY --from=build-env --chown=0:10001 --chmod=454 /src/target/release/signup-sequencer /bin/signup-sequencer

ENTRYPOINT [ "/bin/signup-sequencer" ]
