FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY src/ src/
COPY schema.sql .
RUN cargo build --release --bin name-a-woman

FROM registry.access.redhat.com/ubi10/ubi-minimal
COPY --from=builder /app/target/release/name-a-woman /usr/local/bin/
COPY static/ /app/static/
COPY schema.sql /app/
WORKDIR /app
ENV RUST_LOG=info
ENV NAW_MAX_FALLBACK_LOOKUPS=20
ENV NAW_INACTIVITY_TIMEOUT_SECS=300
VOLUME ["/app/data"]
EXPOSE 8080
CMD ["name-a-woman"]
