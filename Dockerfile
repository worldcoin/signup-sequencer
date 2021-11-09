FROM rust as build-env
WORKDIR /src

ARG TARGET=x86_64-unknown-linux-musl
ENV TARGET $TARGET

# Build tools for a static musl target
RUN apt-get update &&\
    apt-get install -yq build-essential musl-dev musl-tools libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/* &&\
    rustup target add $TARGET

# Use Mimalloc by default instead of the musl malloc
ARG FEATURES="mimalloc"

# Build dependencies only
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/cli &&\
    echo 'fn main() { }' > build.rs &&\
    echo 'fn main() { panic!("build failed") }' > src/cli/main.rs &&\
    echo '' > src/lib.rs &&\
    cargo build --release --locked --target $TARGET --features "${FEATURES}" --bin rust-app &&\
    rm -r build.rs src

# Take build identifying information as arguments
ARG COMMIT_SHA=0000000000000000000000000000000000000000
ARG COMMIT_DATE=0000-00-00
ENV COMMIT_SHA $COMMIT_SHA
ENV COMMIT_DATE $COMMIT_DATE
ENV BIN="./target/$TARGET/release/rust-app"

# Build app
COPY build.rs Readme.md ./
COPY src ./src
RUN touch build.rs src/lib.rs src/cli/main.rs &&\
    cargo build --release --locked --target $TARGET --features "${FEATURES}" --bin rust-app &&\
    strip $BIN

# Set capabilities
RUN sudo setcap cap_net_bind_service=+ep $BIN

# Make sure it is statically linked
RUN ! ldd $BIN
RUN file $BIN | grep "statically linked"

# Make sure it runs
RUN $BIN --version

# Fetch latest certificates
RUN sudo update-ca-certificates --verbose

################################################################################
# Create minimal docker image for our app
FROM scratch

# Drop priviliges
USER 1000:1000

# Configure SSL CA certificates
# TODO: --chmod=040
COPY --from=build-env --chown=0:1000 \
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
# TODO: --chmod=010
COPY --from=build-env --chown=0:1000 \
    /home/rust/src/target/x86_64-unknown-linux-musl/release/rust-app /
STOPSIGNAL SIGTERM
HEALTHCHECK NONE
ENTRYPOINT ["/rust-app"]
