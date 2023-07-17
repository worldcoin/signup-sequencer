FROM debian:11 as build-env

WORKDIR /src

# Copy all the source files
# .dockerignore ignores the target dir
# This includes the rust-toolchain.toml
COPY . .

# Install dependencies
RUN apt-get update && \
    apt-get install -y curl build-essential libssl-dev texinfo libcap2-bin pkg-config

# TODO: Use a specific version of rustup
# Install rustup
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y

# Set environment variables
ENV PATH="/root/.cargo/bin:${PATH}"
ENV RUSTUP_HOME="/root/.rustup"
ENV CARGO_HOME="/root/.cargo"

# Install the toolchain
RUN rustup component add cargo

# Build the sequencer
RUN cargo build --release --features mimalloc,oz-provider

# Make sure it runs
RUN /src/target/release/signup-sequencer --version

# cc variant because we need libgcc and others
FROM gcr.io/distroless/cc-debian11:nonroot

# Copy the sequencer binary
COPY --from=build-env --chown=0:10001 --chmod=001 /src/target/release/signup-sequencer /bin/signup-sequencer

ENTRYPOINT [ "/bin/signup-sequencer" ]
