#!/bin/bash
set -e

function build-linux() {
    # Create output directory
    mkdir -p dist/linux dist/windows
    docker build --progress=plain --target linux-builder -t ws-forwarder-linux .
    echo "Extracting Linux binaries..."
    docker create --name temp-linux ws-forwarder-linux
    docker cp temp-linux:/workspace/target/release/ws-forwarder-server dist/linux/
    docker cp temp-linux:/workspace/target/release/ws-forwarder-client dist/linux/
    docker rm temp-linux
    echo "Linux binaries:   dist/linux/"
}

function build-windows() {
    # Create output directory
    mkdir -p dist/windows
    docker build --progress=plain --target windows-builder -t ws-forwarder-windows .
    echo "Extracting Windows binaries..."
    docker create --name temp-windows ws-forwarder-windows
    docker cp temp-windows:/workspace/target/x86_64-pc-windows-gnu/release/ws-forwarder-server.exe dist/windows/
    docker cp temp-windows:/workspace/target/x86_64-pc-windows-gnu/release/ws-forwarder-client.exe dist/windows/
    docker rm temp-windows
    echo "Windows binaries: dist/windows/"
}

if [ "$1" == "linux" ]; then
    build-linux
    exit 0
elif [ "$1" == "windows" ]; then
    build-windows
    exit 0
fi
echo "Building both Linux and Windows binaries..."
build-linux
build-windows
