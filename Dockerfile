# Stage 1: Build statically linked Linux binary (musl, no glibc dependency)
FROM rust:1-trixie AS linux-builder

WORKDIR /workspace
COPY . .

ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl musl-tools perl make && \
    rm -rf /var/lib/apt/lists/* && \
    curl -fsSL -o /usr/local/share/ca-certificates/corp-root-ca.crt http://it-services.dermalog.hh/DERMALOG-CA012-2029.crt || true && \
    update-ca-certificates || true && \
    rustup target add x86_64-unknown-linux-musl && \
    cargo build --release --target x86_64-unknown-linux-musl && \
    strip target/x86_64-unknown-linux-musl/release/ws-forwarder-server && \
    strip target/x86_64-unknown-linux-musl/release/ws-forwarder-client

# Stage 2: Build Windows binary
FROM rust:1-trixie AS windows-builder

WORKDIR /workspace
COPY . .

RUN apt-get update && \
    apt-get install -y --no-install-recommends mingw-w64 ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    curl -fsSL -o /usr/local/share/ca-certificates/corp-root-ca.crt http://it-services.dermalog.hh/DERMALOG-CA012-2029.crt || true && \
    update-ca-certificates || true && \
    rustup target add x86_64-pc-windows-gnu && \
    cargo build --release --target x86_64-pc-windows-gnu && \
    strip target/x86_64-pc-windows-gnu/release/ws-forwarder-server.exe && \
    strip target/x86_64-pc-windows-gnu/release/ws-forwarder-client.exe
