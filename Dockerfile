FROM rust as build-env
WORKDIR /src

RUN case $(dpkg --print-architecture) in \
    amd64) echo TARGET=x86_64-unknown-linux-musl >> ~/.env ;;\
    arm64) echo TARGET=aarch64-unknown-linux-musl >> ~/.env ;;\
    *) echo Unsupported architecture && false ;;\
    esac

# Build tools for a static musl target
RUN export $(xargs < ~/.env) &&\
    apt-get update &&\
    apt-get install -yq build-essential musl-dev musl-tools libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/* &&\
    rustup target add $TARGET
RUN mkdir -p /usr/local/musl/include
ENV C_INCLUDE_PATH=/usr/local/musl/include
ENV CC=musl-gcc

# Build OpenSSL
ARG OPENSSL_VERSION=1.1.1l
RUN cd /tmp && \
    curl -fLO "https://www.openssl.org/source/openssl-$OPENSSL_VERSION.tar.gz" && \
    tar xzf "openssl-$OPENSSL_VERSION.tar.gz" && cd "openssl-$OPENSSL_VERSION" && \
    cd /tmp/openssl-$OPENSSL_VERSION &&\
    ln -s /usr/include/linux /usr/local/musl/include/linux &&\
    ln -s /usr/include/aarch64-linux-musl/asm /usr/local/musl/include/asm &&\
    ln -s /usr/include/asm-generic /usr/local/musl/include/asm-generic &&\
    ls -l /usr/local/musl/include &&\
    ls -l /usr/local/musl/include/linux &&\
    cat /usr/local/musl/include/linux/version.h &&\
    ./Configure no-shared no-zlib -fPIC --prefix=/usr/local/musl -DOPENSSL_NO_SECURE_MEMORY linux-aarch64 &&\
    make depend && make && make install_sw &&\
    rm /usr/local/musl/include/linux /usr/local/musl/include/asm /usr/local/musl/include/asm-generic &&\
    rm -r /tmp/*

# Build zlib
ARG ZLIB_VERSION=1.2.11
RUN cd /tmp && \
    curl -fLO "http://zlib.net/zlib-$ZLIB_VERSION.tar.gz" && \
    tar xzf "zlib-$ZLIB_VERSION.tar.gz" && cd "zlib-$ZLIB_VERSION" && \
    ./configure --static --prefix=/usr/local/musl && \
    make && make install && \
    rm -r /tmp/*

ENV X86_64_UNKNOWN_LINUX_MUSL_OPENSSL_DIR=/usr/local/musl/ \
    X86_64_UNKNOWN_LINUX_MUSL_OPENSSL_STATIC=1 \
    PG_CONFIG_X86_64_UNKNOWN_LINUX_GNU=/usr/bin/pg_config \
    PKG_CONFIG_ALLOW_CROSS=true \
    PKG_CONFIG_ALL_STATIC=true \
    LIBZ_SYS_STATIC=1

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
RUN setcap cap_net_bind_service=+ep $BIN

# Make sure it is statically linked
RUN ldd $BIN ; file $BIN
RUN ldd $BIN | grep "statically linked"
# RUN file $BIN | grep "statically linked"

# Make sure it runs
RUN $BIN --version

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
COPY --from=build-env --chown=0:1000 \
    /src/target/x86_64-unknown-linux-musl/release/rust-app /
STOPSIGNAL SIGTERM
HEALTHCHECK NONE
ENTRYPOINT ["/rust-app"]
