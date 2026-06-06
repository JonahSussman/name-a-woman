FROM registry.access.redhat.com/ubi10/ubi-minimal AS builder
RUN microdnf install -y rust-toolset nodejs npm make && microdnf clean all
WORKDIR /app

# Cache Rust dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin \
    && echo "fn main() {}" > src/main.rs \
    && cp src/main.rs src/bin/import_wikidata.rs \
    && cp src/main.rs src/bin/import_dump.rs \
    && echo "pub mod config; pub mod db; pub mod game; pub mod handlers; pub mod models; pub mod normalize; pub mod session; pub mod wikidata;" > src/lib.rs \
    && for m in config db game handlers models normalize session wikidata; do touch src/$m.rs; done \
    && cargo build --release \
    && rm -rf src target/release/name-a-woman target/release/deps/*name_a_woman*

# Install frontend dependencies
COPY frontend/package.json frontend/package-lock.json frontend/
RUN cd frontend && npm ci

# Build everything
COPY src/ src/
COPY frontend/ frontend/
COPY Makefile .
RUN make release

FROM registry.access.redhat.com/ubi10/ubi-minimal
COPY --from=builder /app/target/release/name-a-woman /usr/local/bin/
COPY --from=builder /app/static/ /app/static/
WORKDIR /app
ENV RUST_LOG=info
ENV NAW_DB_PATH=data/names.db
ENV NAW_MAX_FALLBACK_LOOKUPS=500
ENV NAW_INACTIVITY_TIMEOUT_SECS=300
VOLUME ["/app/data"]
EXPOSE 8080
CMD ["name-a-woman"]
