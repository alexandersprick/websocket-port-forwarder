#!/bin/bash
set -e

echo "Building Linux and Windows binaries in Docker..."

# Build both Linux and Windows builders
docker build --target linux-builder -t rust-reverse-tunnel-linux .
docker build --target windows-builder -t rust-reverse-tunnel-windows .

# Create output directory
mkdir -p dist/linux dist/windows

# Extract Linux binaries
echo "Extracting Linux binaries..."
docker create --name temp-linux rust-reverse-tunnel-linux
docker cp temp-linux:/workspace/target/release/tunnel-server dist/linux/
docker cp temp-linux:/workspace/target/release/tunnel-client dist/linux/
docker rm temp-linux

# Extract Windows binaries
echo "Extracting Windows binaries..."
docker create --name temp-windows rust-reverse-tunnel-windows
docker cp temp-windows:/workspace/target/x86_64-pc-windows-gnu/release/tunnel-server.exe dist/windows/
docker cp temp-windows:/workspace/target/x86_64-pc-windows-gnu/release/tunnel-client.exe dist/windows/
docker rm temp-windows

echo ""
echo "Build complete!"
echo "Linux binaries:   dist/linux/"
echo "Windows binaries: dist/windows/"
echo ""
echo "File sizes:"
ls -lh dist/linux/
ls -lh dist/windows/

echo ""
echo "Verify Linux static linking:"
file dist/linux/tunnel-server
ldd dist/linux/tunnel-server 2>&1 || echo "(static binary - no dynamic dependencies)"
