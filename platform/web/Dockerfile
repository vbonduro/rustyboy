# ── Build args ─────────────────────────────────────────────────────────────
# Pass --build-arg DEBUG_OVERLAY=1 to enable the on-screen debug overlay.
# Default is off (production build).
ARG DEBUG_OVERLAY=0

# ── Stage 1: Build WASM client ─────────────────────────────────────────────
FROM rust:1.88-slim AS wasm-builder

ARG DEBUG_OVERLAY

RUN apt-get update && apt-get install -y \
    curl \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Install wasm-pack
RUN curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

WORKDIR /build

# Copy workspace files needed for the WASM crate
COPY Cargo.toml Cargo.lock ./
COPY core ./core
COPY platform/web/client ./platform/web/client
# Stub out server so workspace resolves
COPY platform/web/server/Cargo.toml ./platform/web/server/Cargo.toml
RUN mkdir -p platform/web/server/src && echo "fn main() {}" > platform/web/server/src/main.rs

# Build WASM with wasm-pack (--target web = native ES module, no bundler)
# Pass --features debug-overlay when DEBUG_OVERLAY=1
RUN if [ "$DEBUG_OVERLAY" = "1" ]; then \
      wasm-pack build platform/web/client \
        --target web \
        --out-dir /build/platform/web/client/pkg \
        --release \
        --features debug-overlay; \
    else \
      wasm-pack build platform/web/client \
        --target web \
        --out-dir /build/platform/web/client/pkg \
        --release; \
    fi

# ── Stage 2: Build the Axum server ─────────────────────────────────────────
FROM rust:1.88-slim AS server-builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY core ./core
COPY platform/web/server ./platform/web/server
# Stub out the WASM client so workspace resolves
COPY platform/web/client/Cargo.toml ./platform/web/client/Cargo.toml
RUN mkdir -p platform/web/client/src && echo "// stub" > platform/web/client/src/lib.rs

RUN cargo build --release -p rustyboy-web-server

# ── Stage 3: Runtime image ──────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

# Copy server binary
COPY --from=server-builder \
    /build/target/release/rustyboy-server \
    /usr/local/bin/rustyboy-server

# Copy static assets: HTML/CSS/JS + WASM pkg
COPY platform/web/client/static /static
COPY --from=wasm-builder /build/platform/web/client/pkg /static

# Expose port and set defaults
EXPOSE 8080
ENV ROMS_DIR=/roms
ENV STATIC_DIR=/static
ENV DB_PATH=/appdata/rustyboy.db

# ROMs are mounted at runtime: docker run -v /path/to/roms:/roms
ENTRYPOINT ["/usr/local/bin/rustyboy-server"]
