FROM registry.access.redhat.com/ubi10/ubi-minimal AS frontend
RUN microdnf install -y nodejs npm && microdnf clean all
WORKDIR /app
COPY static/ static/
RUN npm install -g esbuild html-minifier-terser \
    && esbuild static/app.js --minify --outfile=static/app.js --allow-overwrite \
    && esbuild static/style.css --minify --outfile=static/style.css --allow-overwrite \
    && html-minifier-terser --collapse-whitespace --remove-comments --minify-css --minify-js \
       -o static/index.min.html static/index.html \
    && mv static/index.min.html static/index.html

FROM registry.access.redhat.com/ubi10/ubi-minimal AS builder
RUN microdnf install -y rust-toolset && microdnf clean all
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin \
    && echo "fn main() {}" > src/main.rs \
    && cp src/main.rs src/bin/import_wikidata.rs \
    && cp src/main.rs src/bin/import_dump.rs \
    && echo "pub mod config; pub mod db; pub mod game; pub mod handlers; pub mod models; pub mod normalize; pub mod session; pub mod wikidata;" > src/lib.rs \
    && for m in config db game handlers models normalize session wikidata; do touch src/$m.rs; done \
    && cargo build --release \
    && rm -rf src target/release/name-a-woman target/release/deps/*name_a_woman*
COPY src/ src/
COPY schema.sql .
RUN cargo build --release --bin name-a-woman

FROM registry.access.redhat.com/ubi10/ubi-minimal
COPY --from=builder /app/target/release/name-a-woman /usr/local/bin/
COPY --from=frontend /app/static/ /app/static/
COPY schema.sql /app/
WORKDIR /app
ENV RUST_LOG=info
ENV NAW_DB_PATH=data/names.db
ENV NAW_MAX_FALLBACK_LOOKUPS=20
ENV NAW_INACTIVITY_TIMEOUT_SECS=300
VOLUME ["/app/data"]
EXPOSE 8080
CMD ["name-a-woman"]
