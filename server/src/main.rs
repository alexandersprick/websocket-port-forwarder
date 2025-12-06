use anyhow::{Context, Result};
use clap::Parser;
use common::Message;
use futures_util::{SinkExt, StreamExt};
use native_tls::{Identity, TlsAcceptor};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tokio_native_tls::TlsAcceptor as TokioTlsAcceptor;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "ws-forwarder-server")]
#[command(about = "Reverse tunnel server over WebSocket", long_about = None)]
#[command(version)]
struct Args {
    /// WebSocket server bind address
    #[arg(short, long, default_value = "0.0.0.0:8443")]
    bind: String,

    /// TLS certificate file (PKCS12/PFX format). If not provided, TLS will be disabled (use when behind a TLS proxy)
    #[arg(short, long)]
    cert: Option<String>,

    /// TLS certificate password
    #[arg(short, long, default_value = "")]
    password: String,
}

type ClientId = usize;
type TunnelId = u32;
type RemotePort = u16;

struct ClientConnection {
    sender: mpsc::UnboundedSender<Message>,
    registered_ports: Vec<RemotePort>,
    listener_tasks: Vec<tokio::task::JoinHandle<()>>,
}

struct ServerState {
    clients: RwLock<HashMap<ClientId, ClientConnection>>,
    port_to_client: RwLock<HashMap<RemotePort, ClientId>>,
    next_client_id: RwLock<ClientId>,
    next_tunnel_id: RwLock<TunnelId>,
    tunnel_senders: RwLock<HashMap<TunnelId, mpsc::UnboundedSender<Vec<u8>>>>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            port_to_client: RwLock::new(HashMap::new()),
            next_client_id: RwLock::new(0),
            next_tunnel_id: RwLock::new(0),
            tunnel_senders: RwLock::new(HashMap::new()),
        }
    }

    async fn allocate_client_id(&self) -> ClientId {
        let mut id = self.next_client_id.write().await;
        let current = *id;
        *id += 1;
        current
    }

    async fn allocate_tunnel_id(&self) -> TunnelId {
        let mut id = self.next_tunnel_id.write().await;
        let current = *id;
        *id += 1;
        current
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    
    // Load TLS certificate if provided
    let tls_acceptor = if let Some(cert_path) = &args.cert {
        let mut file = File::open(cert_path)
            .context("Failed to open certificate file")?;
        let mut identity_data = vec![];
        file.read_to_end(&mut identity_data)
            .context("Failed to read certificate")?;
        
        let identity = Identity::from_pkcs12(&identity_data, &args.password)
            .context("Failed to parse certificate")?;
        
        let acceptor = TlsAcceptor::new(identity)
            .context("Failed to create TLS acceptor")?;
        Some(Arc::new(acceptor))
    } else {
        info!("TLS disabled - running without encryption (use only behind a TLS proxy)");
        None
    };

    let state = Arc::new(ServerState::new());
    
    let listener = TcpListener::bind(&args.bind).await?;
    let protocol = if tls_acceptor.is_some() { "wss" } else { "ws" };
    info!("Server listening on {}://{}", protocol, args.bind);

    while let Ok((stream, addr)) = listener.accept().await {
        let tls_acceptor = tls_acceptor.clone();
        let state = state.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, addr, tls_acceptor, state).await {
                error!("Connection error from {}: {}", addr, e);
            }
        });
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    tls_acceptor: Option<Arc<TlsAcceptor>>,
    state: Arc<ServerState>,
) -> Result<()> {
    info!("New connection from {}", addr);

    if let Some(acceptor) = tls_acceptor {
        // Handle TLS connection
        let tls_stream = TokioTlsAcceptor::from(acceptor.as_ref().clone())
            .accept(stream)
            .await
            .context("TLS handshake failed")?;
        let ws_stream = accept_async(tls_stream)
            .await
            .context("WebSocket handshake failed")?;
        handle_websocket(ws_stream, addr, state).await
    } else {
        // Handle non-TLS connection
        let ws_stream = accept_async(stream)
            .await
            .context("WebSocket handshake failed")?;
        handle_websocket(ws_stream, addr, state).await
    }
}

async fn handle_websocket<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    addr: SocketAddr,
    state: Arc<ServerState>,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    
    info!("WebSocket connection established from {}", addr);

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let client_id = state.allocate_client_id().await;
    
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();
    
    // Register client
    {
        let mut clients = state.clients.write().await;
        clients.insert(client_id, ClientConnection {
            sender: msg_tx.clone(),
            registered_ports: Vec::new(),
            listener_tasks: Vec::new(),
        });
    }

    info!("Client {} registered", client_id);

    // Spawn task to send messages to client
    let send_task = tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            if let Ok(data) = msg.to_bytes() {
                if ws_sender.send(WsMessage::Binary(data)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle incoming messages
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(WsMessage::Binary(data)) => {
                if let Ok(message) = Message::from_bytes(&data) {
                    handle_client_message(client_id, message, &state).await?;
                }
            }
            Ok(WsMessage::Close(_)) => {
                info!("Client {} closed connection", client_id);
                break;
            }
            Ok(WsMessage::Ping(_data)) => {
                // Pong is automatically sent by tungstenite
                continue;
            }
            Err(e) => {
                error!("WebSocket error from client {}: {}", client_id, e);
                break;
            }
            _ => {}
        }
    }

    // Cleanup
    cleanup_client(client_id, &state).await;
    send_task.abort();

    Ok(())
}

async fn handle_client_message(
    client_id: ClientId,
    message: Message,
    state: &Arc<ServerState>,
) -> Result<()> {
    match message {
        Message::Register { local_port, remote_port } => {
            info!("Client {} registering port {} -> {}", client_id, remote_port, local_port);
            
            // Check if port is already registered
            let port_map = state.port_to_client.read().await;
            if port_map.contains_key(&remote_port) {
                let clients = state.clients.read().await;
                if let Some(client) = clients.get(&client_id) {
                    let _ = client.sender.send(Message::RegisterAck {
                        remote_port,
                        success: false,
                        error: Some("Port already in use".to_string()),
                    });
                }
                return Ok(());
            }
            drop(port_map);

            // Register port
            {
                let mut port_map = state.port_to_client.write().await;
                port_map.insert(remote_port, client_id);
                
                let mut clients = state.clients.write().await;
                if let Some(client) = clients.get_mut(&client_id) {
                    client.registered_ports.push(remote_port);
                }
            }

            // Start listening on the remote port
            let state_clone = state.clone();
            let listener_task = tokio::spawn(async move {
                if let Err(e) = listen_on_port(remote_port, client_id, state_clone).await {
                    error!("Failed to listen on port {}: {}", remote_port, e);
                }
            });

            // Store the task handle
            {
                let mut clients = state.clients.write().await;
                if let Some(client) = clients.get_mut(&client_id) {
                    client.listener_tasks.push(listener_task);
                }
            }

            // Send acknowledgment
            let clients = state.clients.read().await;
            if let Some(client) = clients.get(&client_id) {
                let _ = client.sender.send(Message::RegisterAck {
                    remote_port,
                    success: true,
                    error: None,
                });
            }
        }
        Message::TunnelOpenAck { tunnel_id, success, error } => {
            if !success {
                warn!("Client {} failed to open tunnel {}: {:?}", client_id, tunnel_id, error);
                // Close the tunnel sender
                let mut senders = state.tunnel_senders.write().await;
                senders.remove(&tunnel_id);
            }
        }
        Message::TunnelData { tunnel_id, data } => {
            // Forward data to the tunnel
            let senders = state.tunnel_senders.read().await;
            if let Some(sender) = senders.get(&tunnel_id) {
                let _ = sender.send(data);
            }
        }
        Message::TunnelClose { tunnel_id } => {
            info!("Client {} closed tunnel {}", client_id, tunnel_id);
            let mut senders = state.tunnel_senders.write().await;
            senders.remove(&tunnel_id);
        }
        Message::Ping => {
            // Respond with Pong for keepalive
            let clients = state.clients.read().await;
            if let Some(client) = clients.get(&client_id) {
                let _ = client.sender.send(Message::Pong);
            }
        }
        Message::Pong => {
            // Keepalive response
        }
        _ => {
            warn!("Unexpected message from client {}: {:?}", client_id, message);
        }
    }
    Ok(())
}

async fn listen_on_port(
    port: RemotePort,
    client_id: ClientId,
    state: Arc<ServerState>,
) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on {} for client {}", addr, client_id);

    loop {
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                info!("New connection on port {} from {}", port, peer_addr);
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_tunnel(stream, port, client_id, state).await {
                        error!("Tunnel error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Accept error on port {}: {}", port, e);
                break;
            }
        }
    }

    Ok(())
}

async fn handle_tunnel(
    stream: TcpStream,
    port: RemotePort,
    client_id: ClientId,
    state: Arc<ServerState>,
) -> Result<()> {
    let tunnel_id = state.allocate_tunnel_id().await;
    
    // Create channel for receiving data from client
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    
    {
        let mut senders = state.tunnel_senders.write().await;
        senders.insert(tunnel_id, tx);
    }

    // Request client to open tunnel
    {
        let clients = state.clients.read().await;
        if let Some(client) = clients.get(&client_id) {
            client.sender.send(Message::TunnelOpen {
                tunnel_id,
                remote_port: port,
            })?;
        } else {
            return Err(anyhow::anyhow!("Client not found"));
        }
    }

    info!("Tunnel {} opened for port {}", tunnel_id, port);

    // Spawn task to read from TCP and send to client
    let state_clone = state.clone();
    let (mut stream_read, mut stream_write) = stream.into_split();
    let read_task = tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        loop {
            match stream_read.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = buffer[..n].to_vec();
                    let clients = state_clone.clients.read().await;
                    if let Some(client) = clients.get(&client_id) {
                        if client.sender.send(Message::TunnelData {
                            tunnel_id,
                            data,
                        }).is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        
        // Send close message
        let clients = state_clone.clients.read().await;
        if let Some(client) = clients.get(&client_id) {
            let _ = client.sender.send(Message::TunnelClose { tunnel_id });
        }
    });

    // Write data from client to TCP
    while let Some(data) = rx.recv().await {
        if stream_write.write_all(&data).await.is_err() {
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

async fn cleanup_client(client_id: ClientId, state: &Arc<ServerState>) {
    info!("Cleaning up client {}", client_id);
    
    // Remove client and get their data
    let (ports, listener_tasks) = {
        let mut clients = state.clients.write().await;
        if let Some(client) = clients.remove(&client_id) {
            (client.registered_ports, client.listener_tasks)
        } else {
            (Vec::new(), Vec::new())
        }
    };

    // Stop all listener tasks
    for task in listener_tasks {
        task.abort();
    }

    // Remove port mappings
    let mut port_map = state.port_to_client.write().await;
    for port in ports {
        port_map.remove(&port);
        info!("Released port {} from client {}", port, client_id);
    }
}
