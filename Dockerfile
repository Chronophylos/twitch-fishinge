# Using the `rust-musl-builder` as base image, instead of 
# the official Rust toolchain
FROM clux/muslrust:1.63.0 AS chef
USER root
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --target x86_64-unknown-linux-musl --release --recipe-path recipe.json
# Build application
COPY . .
RUN SQLX_OFFLINE=true cargo build --target x86_64-unknown-linux-musl --release --bin twitch-fishinge

# We do not need the Rust toolchain to run the binary!
FROM alpine AS runtime
RUN addgroup -S myuser && adduser -S myuser -G myuser
USER myuser
WORKDIR /app
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/twitch-fishinge /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/twitch-fishinge"]
