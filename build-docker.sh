#!/usr/bin/env bash
set -euo pipefail

build_linux() {
    mkdir -p dist/linux
    docker build --progress=plain --target linux-builder -t ws-forwarder-linux .
    echo "Extracting Linux binaries..."
    docker rm -f temp-linux >/dev/null 2>&1 || true
    docker create --name temp-linux ws-forwarder-linux
    docker cp temp-linux:/workspace/target/x86_64-unknown-linux-musl/release/ws-forwarder-server dist/linux/
    docker cp temp-linux:/workspace/target/x86_64-unknown-linux-musl/release/ws-forwarder-client dist/linux/
    docker rm -f temp-linux
    echo "Linux binaries: dist/linux/"
}

build_windows() {
    mkdir -p dist/windows
    docker build --progress=plain --target windows-builder -t ws-forwarder-windows .
    echo "Extracting Windows binaries..."
    docker rm -f temp-windows >/dev/null 2>&1 || true
    docker create --name temp-windows ws-forwarder-windows
    docker cp temp-windows:/workspace/target/x86_64-pc-windows-gnu/release/ws-forwarder-server.exe dist/windows/
    docker cp temp-windows:/workspace/target/x86_64-pc-windows-gnu/release/ws-forwarder-client.exe dist/windows/
    docker rm -f temp-windows
    echo "Windows binaries: dist/windows/"
}

case "${1:-all}" in
    linux)
        build_linux
        ;;
    windows)
        build_windows
        ;;
    all|"")
        echo "Building both Linux and Windows binaries..."
        build_linux
        build_windows
        ;;
    *)
        echo "Usage: $0 [linux|windows|all]"
        exit 1
        ;;
esac
