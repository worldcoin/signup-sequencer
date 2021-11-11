FROM rust as build-env
WORKDIR /src

RUN apt-get update &&\
    apt-get install -y texinfo libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/* &&\
    rustup target add $(uname -m)-unknown-linux-musl

# Build {x86_64,aarch64}-linux-musl toolchain
# This is required to build zlib, openssl and other C dependencies
ARG MUSL_CROSS_VERSION=0.9.9
RUN curl -fL "https://github.com/richfelker/musl-cross-make/archive/v${MUSL_CROSS_VERSION}.tar.gz"\
    | tar xz && cd musl-cross-make-${MUSL_CROSS_VERSION} &&\
    make install TARGET=$(uname -m)-linux-musl OUTPUT=/usr/local/musl &&\
    rm -r /src/musl-cross-make-${MUSL_CROSS_VERSION}

# Build zlib
ARG ZLIB_VERSION=1.2.11
RUN curl -fL "http://zlib.net/zlib-$ZLIB_VERSION.tar.gz" | tar xz && cd "zlib-$ZLIB_VERSION" &&\
    export CC=/usr/local/musl/bin/$(uname -m)-linux-musl-gcc &&\
    ./configure --static --prefix=/usr/local/musl && make && make install &&\
    rm -r "/src/zlib-$ZLIB_VERSION"

# Build OpenSSL
ARG OPENSSL_VERSION=1.1.1l
RUN curl -fL "https://www.openssl.org/source/openssl-$OPENSSL_VERSION.tar.gz" | tar xz &&\
    cd "openssl-$OPENSSL_VERSION" &&\
    export CC=/usr/local/musl/bin/$(uname -m)-linux-musl-gcc &&\
    ./Configure no-shared --prefix=/usr/local/musl linux-$(uname -m) &&\
    make install_sw &&\
    rm -r "/src/openssl-$OPENSSL_VERSION"
ENV OPENSSL_DIR=/usr/local/musl
ENV OPENSSL_STATIC=1

# Use Mimalloc by default instead of the musl malloc
# TODO ARG FEATURES="mimalloc"
ARG FEATURES=""

# Build dependencies only
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/cli &&\
    echo 'fn main() { }' > build.rs &&\
    echo 'fn main() { panic!("build failed") }' > src/cli/main.rs &&\
    echo '' > src/lib.rs &&\
    export PATH="/usr/local/musl/bin:$PATH" &&\
    cargo build --locked --release --target $(uname -m)-unknown-linux-musl --features "${FEATURES}" --bin rust-app &&\
    rm -r build.rs src

# Take build identifying information as arguments
ARG COMMIT_SHA=0000000000000000000000000000000000000000
ARG COMMIT_DATE=0000-00-00
ENV COMMIT_SHA $COMMIT_SHA
ENV COMMIT_DATE $COMMIT_DATE

# Build app
ARG BIN=rust-app
COPY build.rs Readme.md ./
COPY src ./src
RUN touch build.rs src/lib.rs src/cli/main.rs &&\
    cargo build --locked --release --target $(uname -m)-unknown-linux-musl --features "${FEATURES}" --bin $BIN &&\
    cp ./target/$(uname -m)-unknown-linux-musl/release/$BIN ./bin &&\
    strip ./bin

# Set capabilities
RUN setcap cap_net_bind_service=+ep ./bin

# Make sure it is statically linked
RUN ldd ./bin | grep "statically linked"
RUN file ./bin | grep "statically linked"

# Make sure it runs
RUN ./bin --version

# Fetch latest certificates
RUN update-ca-certificates --verbose

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
COPY --from=build-env --chown=0:1000 /src/bin /bin
STOPSIGNAL SIGTERM
HEALTHCHECK NONE
ENTRYPOINT ["/bin"]
