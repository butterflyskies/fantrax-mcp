# syntax=docker/dockerfile:1

# ─── Build stage ────────────────────────────────────────────────────────────
FROM rust:1.87-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build

# Cache dependency build: copy only manifests first.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

# Build the real binary.
COPY src/ src/
# Touch main.rs so cargo rebuilds it (not the cached dummy).
RUN touch src/main.rs src/lib.rs
RUN cargo build --release --bin fantrax-mcp

# ─── Runtime stage ──────────────────────────────────────────────────────────
FROM alpine:3.21

RUN apk add --no-cache ca-certificates

RUN adduser -D -u 1000 fantrax
USER fantrax

COPY --from=builder /build/target/release/fantrax-mcp /usr/local/bin/fantrax-mcp
COPY config.toml.example /config.toml.example

EXPOSE 3001

ENTRYPOINT ["fantrax-mcp"]
CMD ["--config", "/config.toml"]
