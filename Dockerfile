# Stage 1: Build Linux binary (dynamic linking is fine)
FROM rust:1-trixie AS linux-builder

WORKDIR /workspace
COPY . .

# Build standard Linux binary
RUN cargo build --release && \
    strip target/release/tunnel-server && \
    strip target/release/tunnel-client

# Stage 2: Build Windows binary
FROM rust:1-trixie AS windows-builder

# Install mingw cross-compiler
RUN apt-get update && apt-get install -y \
    mingw-w64 \
    && rm -rf /var/lib/apt/lists/*

# Add Windows target
RUN rustup target add x86_64-pc-windows-gnu

WORKDIR /workspace
COPY . .

# Build Windows binary
RUN cargo build --release --target x86_64-pc-windows-gnu && \
    strip target/x86_64-pc-windows-gnu/release/tunnel-server.exe && \
    strip target/x86_64-pc-windows-gnu/release/tunnel-client.exe
