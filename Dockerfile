FROM rust:1.67 as build-env
WORKDIR /src

RUN apt-get update &&\
    apt-get install -y libssl-dev texinfo libcap2-bin &&\
    apt-get clean && rm -rf /var/lib/apt/lists/*

# Which directory under target to fetch the binary from
ARG PROFILE=debug

# Name of the binary
ARG BIN=signup-sequencer

# Copy target
COPY ./target ./target

# Copy the binary
RUN cp ./target/${PROFILE}/${BIN} ./bin

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
