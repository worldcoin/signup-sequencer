FROM rust:1.59 as build-env
WORKDIR /src

RUN apt-get update &&\
    apt-get install -y libssl-dev texinfo libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/*

ARG BIN=rust-app

# Copy over all releases
COPY ./target ./target

# Select the binary for currenct architecture
RUN cp ./target/$(uname -m)-unknown-linux-musl/release/${BIN} ./bin

# Set capabilities
RUN setcap cap_net_bind_service=+ep ./bin

# Make sure it runs
RUN ./bin --version 

# Fetch latest certificates
RUN update-ca-certificates --verbose

################################################################################
# Create minimal docker image for our app
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
