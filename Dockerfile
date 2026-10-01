# ==============================================================================
# Stage 1: Build the Rust binary
# ==============================================================================
FROM rust:1.80-slim-bookworm AS builder

WORKDIR /usr/src/noide-server

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace sources
COPY . .

# Build release binary for noide-server
RUN cargo build --release --bin noide-server

# ==============================================================================
# Stage 2: Runtime image
# ==============================================================================
FROM debian:bookworm-slim

# Install system developer utilities, git, python3, nodejs, and npm
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    wget \
    git \
    bash \
    procps \
    python3 \
    python3-pip \
    build-essential \
    nodejs \
    npm \
    openssh-client \
    && rm -rf /var/lib/apt/lists/*

# Pre-install AI agent CLIs (kilo, opencode, nio-ai)
RUN npm install -g @kilocode/cli opencode-ai nio-ai

# Also ensure native nio binary is installed in /usr/local/bin
RUN curl -fsSL https://raw.githubusercontent.com/nio-labs/nio/main/install.sh | bash || true

# Copy compiled noide-server binary
COPY --from=builder /usr/src/noide-server/target/release/noide-server /usr/local/bin/noide-server

# Set up persistent workspace volume
RUN mkdir -p /workspace /root/.local/bin /root/.nio/bin
WORKDIR /workspace

# Default environment configuration
ENV PORT=1421 \
    NOTERM_WS_ADDR=0.0.0.0:1421 \
    NOIDE_NO_CLOUDFLARE=true \
    HOME=/root \
    PATH=/root/.local/bin:/root/.nio/bin:/usr/local/bin:$PATH

EXPOSE 1421

CMD ["noide-server"]
