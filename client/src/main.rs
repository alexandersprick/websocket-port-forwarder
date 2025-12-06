use anyhow::{Context, Result};
use clap::Parser;
use common::Message;
use futures_util::{SinkExt, StreamExt};
use native_tls::TlsConnector;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::connect_async_tls_with_config;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "ws-forwarder-client")]
#[command(about = "Reverse tunnel client over WebSocket", long_about = None)]
#[command(version)]
struct Args {
    /// Server WebSocket URL (wss://host:port)
    #[arg(short, long)]
    server: String,

    /// Port forwarding rules: local_port:remote_port
    /// Example: 8080:9000 (forward local 8080 to server's public port 9000)
    #[arg(short, long, value_delimiter = ',')]
    forward: Vec<String>,

    /// Allow invalid TLS certificates (insecure, for testing)
    #[arg(long, default_value = "false")]
    insecure: bool,

    /// Retry interval in seconds when connection fails
    #[arg(long, default_value = "60")]
    retry_interval: u64,

    /// Suppress all output
    #[arg(short, long, default_value = "false")]
    quiet: bool,
}

type TunnelId = u32;

struct ForwardRule {
    local_port: u16,
    remote_port: u16,
}

struct ClientState {
    forward_rules: Vec<ForwardRule>,
    tunnel_senders: RwLock<HashMap<TunnelId, mpsc::UnboundedSender<Vec<u8>>>>,
}

impl ClientState {
    fn new(forward_rules: Vec<ForwardRule>) -> Self {
        Self {
            forward_rules,
            tunnel_senders: RwLock::new(HashMap::new()),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if !args.quiet {
        tracing_subscriber::fmt::init();
    }

    // Parse forwarding rules
    let mut forward_rules = Vec::new();
    for rule in &args.forward {
        let parts: Vec<&str> = rule.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!("Invalid forward rule: {}. Use format local_port:remote_port", rule));
        }
        let local_port = parts[0].parse::<u16>()
            .context(format!("Invalid local port in rule: {}", rule))?;
        let remote_port = parts[1].parse::<u16>()
            .context(format!("Invalid remote port in rule: {}", rule))?;
        
        forward_rules.push(ForwardRule { local_port, remote_port });
        info!("Forward rule: local {} -> remote {}", local_port, remote_port);
    }

    if forward_rules.is_empty() {
        return Err(anyhow::anyhow!("No forwarding rules specified. Use --forward local:remote"));
    }

    let state = Arc::new(ClientState::new(forward_rules));
    let retry_interval = Duration::from_secs(args.retry_interval);

    // Connect to server with TLS
    let tls_connector = if args.insecure {
        TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()?
    } else {
        TlsConnector::new()?
    };

    loop {
        info!("Connecting to {}", args.server);
        
        let ws_stream = match connect_async_tls_with_config(
            &args.server,
            None,
            false,
            Some(tokio_tungstenite::Connector::NativeTls(tls_connector.clone())),
        )
        .await
        {
            Ok((stream, _)) => {
                info!("Connected to server");
                stream
            }
            Err(e) => {
                error!("Failed to connect to server: {}", e);
                info!("Retrying in {} seconds...", args.retry_interval);
                tokio::time::sleep(retry_interval).await;
                continue;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Send registration messages for all forwarding rules
        let mut registration_failed = false;
        for rule in &state.forward_rules {
            let msg = Message::Register {
                local_port: rule.local_port,
                remote_port: rule.remote_port,
            };
            let data = msg.to_bytes()?;
            if let Err(e) = ws_sender.send(WsMessage::Binary(data)).await {
                error!("Failed to register forwarding: {}", e);
                registration_failed = true;
                break;
            }
            info!("Registered forwarding: local {} -> remote {}", rule.local_port, rule.remote_port);
        }

        if registration_failed {
            error!("Registration failed, reconnecting...");
            info!("Retrying in {} seconds...", args.retry_interval);
            tokio::time::sleep(retry_interval).await;
            continue;
        }

        // Spawn keepalive task
        let ws_sender_clone = Arc::new(tokio::sync::Mutex::new(ws_sender));
        let ws_sender_keepalive = ws_sender_clone.clone();
        let keepalive_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let msg = Message::Ping;
                if let Ok(data) = msg.to_bytes() {
                    let mut sender = ws_sender_keepalive.lock().await;
                    if sender.send(WsMessage::Binary(data)).await.is_err() {
                        break;
                    }
                }
            }
        });

        // Handle incoming messages
        let mut connection_interrupted = false;
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(WsMessage::Binary(data)) => {
                    if let Ok(message) = Message::from_bytes(&data) {
                        let ws_sender = ws_sender_clone.clone();
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_server_message(message, state, ws_sender).await {
                                error!("Error handling message: {}", e);
                            }
                        });
                    }
                }
                Ok(WsMessage::Close(_)) => {
                    info!("Server closed connection");
                    connection_interrupted = true;
                    break;
                }
                Ok(WsMessage::Ping(_)) => {
                    // Pong is automatically sent
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    connection_interrupted = true;
                    break;
                }
                _ => {}
            }
        }

        keepalive_task.abort();
        
        // Clear tunnel senders on disconnect
        {
            let mut senders = state.tunnel_senders.write().await;
            senders.clear();
        }

        if connection_interrupted {
            info!("Connection interrupted, retrying in {} seconds...", args.retry_interval);
            tokio::time::sleep(retry_interval).await;
        } else {
            info!("Client shutting down");
            break;
        }
    }

    Ok(())
}

async fn handle_server_message(
    message: Message,
    state: Arc<ClientState>,
    ws_sender: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<TcpStream>
        >,
        WsMessage
    >>>,
) -> Result<()> {
    match message {
        Message::RegisterAck { remote_port, success, error } => {
            if success {
                info!("Successfully registered remote port {}", remote_port);
            } else {
                error!("Failed to register remote port {}: {:?}", remote_port, error);
            }
        }
        Message::TunnelOpen { tunnel_id, remote_port } => {
            info!("Server requesting tunnel {} for port {}", tunnel_id, remote_port);
            
            // Find local port for this remote port
            let local_port = state.forward_rules.iter()
                .find(|r| r.remote_port == remote_port)
                .map(|r| r.local_port);

            if let Some(local_port) = local_port {
                // Create tunnel
                tokio::spawn(async move {
                    if let Err(e) = create_tunnel(tunnel_id, local_port, state, ws_sender).await {
                        error!("Failed to create tunnel {}: {}", tunnel_id, e);
                    }
                });
            } else {
                warn!("No local port configured for remote port {}", remote_port);
                // Send failure acknowledgment
                let msg = Message::TunnelOpenAck {
                    tunnel_id,
                    success: false,
                    error: Some("No local port configured".to_string()),
                };
                let data = msg.to_bytes()?;
                let mut sender = ws_sender.lock().await;
                sender.send(WsMessage::Binary(data)).await?;
            }
        }
        Message::TunnelData { tunnel_id, data } => {
            // Forward data to local connection
            let senders = state.tunnel_senders.read().await;
            if let Some(sender) = senders.get(&tunnel_id) {
                let _ = sender.send(data);
            }
        }
        Message::TunnelClose { tunnel_id } => {
            info!("Server closed tunnel {}", tunnel_id);
            let mut senders = state.tunnel_senders.write().await;
            senders.remove(&tunnel_id);
        }
        Message::Ping => {
            // Send pong
            let msg = Message::Pong;
            let data = msg.to_bytes()?;
            let mut sender = ws_sender.lock().await;
            sender.send(WsMessage::Binary(data)).await?;
        }
        Message::Pong => {
            // Keepalive response from server
        }
        _ => {
            warn!("Unexpected message from server: {:?}", message);
        }
    }
    Ok(())
}

async fn create_tunnel(
    tunnel_id: TunnelId,
    local_port: u16,
    state: Arc<ClientState>,
    ws_sender: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<TcpStream>
        >,
        WsMessage
    >>>,
) -> Result<()> {
    // Connect to local service
    let local_addr = format!("127.0.0.1:{}", local_port);
    let local_stream = match TcpStream::connect(&local_addr).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("Failed to connect to local service {}: {}", local_addr, e);
            
            // Send failure acknowledgment
            let msg = Message::TunnelOpenAck {
                tunnel_id,
                success: false,
                error: Some(format!("Failed to connect to local service: {}", e)),
            };
            let data = msg.to_bytes()?;
            let mut sender = ws_sender.lock().await;
            sender.send(WsMessage::Binary(data)).await?;
            
            return Err(e.into());
        }
    };

    info!("Connected to local service {} for tunnel {}", local_addr, tunnel_id);

    // Send success acknowledgment
    {
        let msg = Message::TunnelOpenAck {
            tunnel_id,
            success: true,
            error: None,
        };
        let data = msg.to_bytes()?;
        let mut sender = ws_sender.lock().await;
        sender.send(WsMessage::Binary(data)).await?;
    }

    // Create channel for receiving data from server
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    
    {
        let mut senders = state.tunnel_senders.write().await;
        senders.insert(tunnel_id, tx);
    }

    // Spawn task to read from local and send to server
    let ws_sender_clone = ws_sender.clone();
    let (mut local_read, mut local_write) = local_stream.into_split();
    let read_task = tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        loop {
            match local_read.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = buffer[..n].to_vec();
                    let msg = Message::TunnelData {
                        tunnel_id,
                        data,
                    };
                    if let Ok(msg_data) = msg.to_bytes() {
                        let mut sender = ws_sender_clone.lock().await;
                        if sender.send(WsMessage::Binary(msg_data)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        
        // Send close message
        let msg = Message::TunnelClose { tunnel_id };
        if let Ok(data) = msg.to_bytes() {
            let mut sender = ws_sender_clone.lock().await;
            let _ = sender.send(WsMessage::Binary(data)).await;
        }
    });

    // Write data from server to local
    while let Some(data) = rx.recv().await {
        if local_write.write_all(&data).await.is_err() {
            break;
        }
    }

    read_task.abort();
    
    // Cleanup
    {
        let mut senders = state.tunnel_senders.write().await;
        senders.remove(&tunnel_id);
    }

    info!("Tunnel {} closed", tunnel_id);

    Ok(())
}
