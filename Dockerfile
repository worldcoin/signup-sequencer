################################### Prepare the recipe ###################################
FROM rust:1.68 as builder

ARG BIN=rust-app

WORKDIR /src

# Install cargo-chef
RUN cargo install cargo-chef

# cargo-chef requires all the Cargo.toml files and Cargo.lock
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

############################# Build dependencies from the recipe #############################
FROM rust:1.68 as oven

ARG TARGET_ARCH=x86_64

WORKDIR /src

# Copy cargo-chef from builder
COPY --from=builder /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef

# Install deps
RUN apt-get update &&\
    apt-get install -y libssl-dev texinfo libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/recipe.json recipe.json
RUN rustup target add ${TARGET_ARCH}-unknown-linux-musl
RUN cargo chef cook --release --target=${TARGET_ARCH}-unknown-linux-musl --recipe-path recipe.json

################################### Build the application ###################################
FROM rust:1.68 as build-env

ARG TARGET_ARCH=x86_64

WORKDIR /src

# Install deps
RUN apt-get update &&\
    apt-get install -y libssl-dev texinfo libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/*

COPY . .
COPY --from=oven /src/target target
COPY --from=oven /usr/local/cargo /usr/local/cargo

# Build the binary
RUN cargo build --release --target=${TARGET_ARCH}-unknown-linux-musl

RUN cp ./target/${TARGET_ARCH}-unknown-linux-musl/release/${BIN} ./bin

# Make sure it runs
RUN ./bin --version

# Set capabilities
RUN setcap cap_net_bind_service=+ep ./bin

# Fetch latest certificates
RUN update-ca-certificates --verbose

################################### Minimal docker image ###################################
FROM scratch

# Drop priviliges
USER 10001:10001

# Configure SSL CA certificates
COPY --from=build-env --chown=0:10001 --chmod=040 \
    /etc/ssl/certs/ca-certificates.crt /
ENV SSL_CERT_FILE="/ca-certificates.crt"

# Configure logging
ENV LOG_FORMAT="json"
ENV LOG_FILTER="info"

# Expose Prometheus
ENV PROMETHEUS="http://0.0.0.0:9998/metrics"
EXPOSE 9998/tcp
LABEL prometheus.io/scrape="true"
LABEL prometheus.io/port="9998"
LABEL prometheus.io/path="/metrics"

# Executable
COPY --from=build-env --chown=0:10001 --chmod=010 /src/bin /bin
STOPSIGNAL SIGTERM
HEALTHCHECK NONE
ENTRYPOINT ["/bin"]
