# WebSocket Port Forwarder

A reverse tunnel implementation over WebSocket that allows exposing local services through a remote server. Built with Rust for high performance and reliability.

## Features

- **Reverse Tunneling**: Expose local services through a remote server without requiring port forwarding
- **WebSocket Protocol**: Uses WebSocket for reliable, bidirectional communication
- **TLS Support**: Optional TLS encryption for secure connections
- **Multiple Port Forwarding**: Forward multiple local ports through a single connection
- **Automatic Reconnection**: Client automatically reconnects on connection failures or interruptions
- **Cross-Platform**: Builds for Linux and Windows
- **Lightweight**: Minimal resource usage with async I/O

## Architecture

The project consists of three components:

- **Server**: Listens for WebSocket connections from clients and accepts incoming TCP connections on registered ports
- **Client**: Connects to the server and forwards traffic to local services
- **Common**: Shared protocol definitions and message types

### How It Works

1. Client connects to the server via WebSocket
2. Client registers port forwarding rules (e.g., forward local port 8080 to remote port 9000)
3. Server listens on the registered remote ports
4. When a connection arrives at the remote port, server creates a tunnel through the WebSocket
5. Client receives the tunnel request and connects to the local service
6. Data flows bidirectionally through the WebSocket tunnel

## Installation

### Building from Source

Requires Rust 1.70 or later.

```bash
cargo build --release
```

Binaries will be in:
- `target/release/ws-forwarder-server`
- `target/release/ws-forwarder-client`

### Building with Docker

Build Linux and Windows binaries using Docker:

```bash
./build-docker.sh
```

Or build specific platforms:

```bash
./build-docker.sh linux    # Linux only
./build-docker.sh windows  # Windows only
```

Binaries will be in:
- `dist/linux/`
- `dist/windows/`

## Usage

### Server

Start the server to accept client connections:

```bash
ws-forwarder-server --bind 0.0.0.0:8443 --cert server.pfx --password mypassword
```

**Options:**
- `-b, --bind <ADDRESS>`: WebSocket server bind address (default: `0.0.0.0:8443`)
- `-c, --cert <FILE>`: TLS certificate file in PKCS12/PFX format (optional, disables TLS if omitted)
- `-p, --password <PASSWORD>`: TLS certificate password (default: empty)
- `--version`: Show version information

**Running without TLS** (e.g., behind a reverse proxy):

```bash
ws-forwarder-server --bind 0.0.0.0:8080
```

### Client

Connect to the server and forward local ports:

```bash
ws-forwarder-client --server wss://example.com:8443 --forward 8080:9000,3000:3001
```

**Options:**
- `-s, --server <URL>`: Server WebSocket URL (e.g., `wss://example.com:8443`)
- `-f, --forward <RULES>`: Port forwarding rules in format `local_port:remote_port` (comma-separated)
- `--insecure`: Allow invalid TLS certificates (for testing only)
- `--retry-interval <SECONDS>`: Retry interval when connection fails (default: 60)
- `-q, --quiet`: Suppress all output
- `--version`: Show version information

**Example:**
Forward local port 8080 to remote port 9000, and local port 3000 to remote port 3001:

```bash
ws-forwarder-client \
  --server wss://my-server.com:8443 \
  --forward 8080:9000,3000:3001
```

Now:
- Traffic to `my-server.com:9000` → forwarded to `localhost:8080`
- Traffic to `my-server.com:3001` → forwarded to `localhost:3000`

### Testing with Self-Signed Certificates

For testing, you can use the `--insecure` flag to skip certificate validation:

```bash
ws-forwarder-client \
  --server wss://localhost:8443 \
  --forward 8080:9000 \
  --insecure
```

## Use Cases

- **Development**: Expose local web servers for testing with external services (webhooks, APIs)
- **IoT/Edge Devices**: Allow remote access to devices behind NAT/firewalls
- **Service Mesh**: Connect services across different networks
- **Reverse Proxy Alternative**: Expose internal services without modifying firewall rules

## Security Considerations

- Always use TLS in production (`wss://`)
- Use valid certificates (avoid `--insecure` in production)
- Restrict server bind address if needed
- Consider authentication mechanisms for production use
- Monitor open ports and connections

## Protocol

The system uses a custom binary protocol over WebSocket with message types:

- `Register`: Client registers a port forwarding rule
- `RegisterAck`: Server acknowledges registration
- `TunnelOpen`: Server requests opening a tunnel
- `TunnelOpenAck`: Client acknowledges tunnel creation
- `TunnelData`: Bidirectional data transfer
- `TunnelClose`: Close a tunnel
- `Ping`/`Pong`: Keepalive messages

## Building TLS Certificates

### Generate a self-signed certificate for testing:

```bash
# Generate private key and certificate
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes

# Convert to PKCS12 format
openssl pkcs12 -export -out server.pfx -inkey key.pem -in cert.pem -password pass:mypassword
```

## License

This project is open source. See LICENSE file for details.

## Version

Current version: 1.0.0

## Contributing

Contributions are welcome! Please submit issues and pull requests on the project repository.
