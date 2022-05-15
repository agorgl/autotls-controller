FROM rust:latest AS chef
# We only pay the installation cost once,
# it will be cached from the second build onwards
RUN cargo install cargo-chef
WORKDIR /app

# Planner image
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Builder image
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

# Runner image
FROM gcr.io/distroless/cc AS runner
WORKDIR /app
COPY --from=builder /app/target/release/autotls-controller /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]
