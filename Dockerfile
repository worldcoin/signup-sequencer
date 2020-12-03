FROM rust:1.48.0 as build-env

# Prepare for static linking
ENV CARGO_BUILD_TARGET x86_64-unknown-linux-musl
RUN apt-get update &&\
    apt-get install -y musl-tools &&\
    rm -rf /var/lib/apt/lists/* &&\
    rustup target add $CARGO_BUILD_TARGET

# Build dependencies only
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
ARG COMMIT_SHA=0000000000000000000000000000000000000000
ARG COMMIT_DATE=0000-00-00
ENV COMMIT_SHA $COMMIT_SHA
ENV COMMIT_DATE $COMMIT_DATE
RUN mkdir src &&\
    echo 'fn main() { }' > build.rs &&\
    echo 'fn main() { panic!("build failed") }' > src/main.rs &&\
    cargo build --release &&\
    rm -r src

# Build app
ENV CARGO_INSTALL_ROOT /usr/local/app
COPY . ./
RUN ls -lah
RUN touch build.rs src/main.rs &&\
    cargo install --locked --no-track --force --path .

# Check version
RUN /usr/local/app/bin/rust-app-template version

# Create empty docker image containing only our app
FROM scratch
COPY --from=build-env /usr/local/app /
WORKDIR /
USER 1000
ENV RUST_LOG info
ENTRYPOINT ["/bin/rust-app-template"]
