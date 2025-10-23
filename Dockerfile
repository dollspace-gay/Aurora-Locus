# Aurora Locus PDS - Production Docker Image
FROM rust:1.75 as builder

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src
COPY migrations ./migrations

# Build for release
RUN cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/aurora-locus /usr/local/bin/aurora-locus

# Copy migrations
COPY migrations /app/migrations

# Create data directory
RUN mkdir -p /data

# Expose port
EXPOSE 3000

# Set environment
ENV RUST_LOG=info
ENV DATABASE_URL=sqlite:///data/aurora-locus.db

# Run the application
CMD ["aurora-locus"]
