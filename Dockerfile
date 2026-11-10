# Multi-stage build for Rust LSP Gateway
FROM rust:1.89 AS builder

# Install system dependencies needed for building
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    clang \
    lld \
    && rm -rf /var/lib/apt/lists/*

# Set Rust compilation environment variables for better performance
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
ENV CARGO_INCREMENTAL=1
# 设置合理的并行编译数量，不设置则让Cargo自动检测
ENV CARGO_BUILD_JOBS=4  
ENV RUSTFLAGS="-C target-cpu=generic -C link-arg=-fuse-ld=lld"

# Create app directory
WORKDIR /app

# Copy Cargo files first for better layer caching
COPY Cargo.toml Cargo.lock ./

# Copy source code to get proper module structure
COPY src ./src
COPY migrations ./migrations

# Create a dummy main.rs to build dependencies without the actual application logic
RUN mv src/main.rs src/main.rs.bak && \
    echo "fn main() {}" > src/main.rs

# Build dependencies only (this will cache all external dependencies)
RUN cargo build --release --locked && \
    rm -rf target/release/deps/lsp_gateway* target/release/lsp-gateway*

# Restore the real main.rs and build the actual application
RUN mv src/main.rs.bak src/main.rs && \
    cargo build --release --locked


# Runtime stage
# FROM rust:1.89
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app user for security
RUN useradd -r -s /bin/false -m -d /app appuser

# Set working directory
WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/lsp-gateway /app/lsp-gateway

# Copy migrations
COPY --from=builder /app/migrations /app/migrations

# Change ownership to app user
RUN chown -R appuser:appuser /app

# Switch to app user
USER appuser

# Expose port (default is 8080 based on README)
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Set default environment variables
ENV RUST_LOG=info
ENV BIND_ADDRESS=0.0.0.0
ENV PORT=8080


# Run the application
CMD ["./lsp-gateway"]
