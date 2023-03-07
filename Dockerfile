# Prepares the layer with `cargo-chef` installed
FROM rust:1.67 AS chef
# We only pay the installation cost once,
# it will be cached from the second build onwards
RUN cargo install cargo-chef

WORKDIR /src

# Prepares the dependencies list to build
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Builds the project
FROM chef AS builder
WORKDIR /src

# Various deps
RUN apt-get update &&\
    apt-get install -y protobuf-compiler libssl-dev texinfo libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/*

# Name of the binary
ARG BIN=signup-sequencer

COPY --from=planner /src/recipe.json recipe.json

# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --recipe-path recipe.json

# Build application
COPY . .
RUN cargo build

# FROM debian:buster-slim AS runtime
# WORKDIR /src
# COPY --from=builder /app/target/release/app /usr/local/bin
# ENTRYPOINT ["/usr/local/bin/app"]

# Copy the binary
RUN cp ./target/debug/${BIN} ./bin

# Set capabilities
RUN setcap cap_net_bind_service=+ep ./bin

# Make sure it runs
RUN ./bin --version

# Fetch latest certificates
RUN update-ca-certificates --verbose

ENV SSL_CERT_FILE="/etc/ssl/certs/ca-certificates.crt"

# Configure logging
ENV LOG_FORMAT="json"
ENV LOG_FILTER="info"

# Expose Prometheus
ENV PROMETHEUS="http://0.0.0.0:9998/metrics"

EXPOSE 9998/tcp

LABEL prometheus.io/scrape="true"
LABEL prometheus.io/port="9998"
LABEL prometheus.io/path="/metrics"

STOPSIGNAL SIGTERM
HEALTHCHECK NONE

ENTRYPOINT [ "./bin" ]
