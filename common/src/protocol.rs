use serde::{Deserialize, Serialize};

/// Protocol messages exchanged between client and server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Client registers a port forwarding rule
    Register {
        local_port: u16,
        remote_port: u16,
    },
    /// Server acknowledges registration
    RegisterAck {
        remote_port: u16,
        success: bool,
        error: Option<String>,
    },
    /// Server requests client to open a new tunnel
    TunnelOpen {
        tunnel_id: u32,
        remote_port: u16,
    },
    /// Client acknowledges tunnel opening
    TunnelOpenAck {
        tunnel_id: u32,
        success: bool,
        error: Option<String>,
    },
    /// Data transfer in a tunnel
    TunnelData {
        tunnel_id: u32,
        data: Vec<u8>,
    },
    /// Close a tunnel
    TunnelClose {
        tunnel_id: u32,
    },
    /// Ping/Pong for keepalive
    Ping,
    Pong,
}

impl Message {
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_bytes(data: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(data)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = Message::Register {
            local_port: 8080,
            remote_port: 9000,
        };
        let bytes = msg.to_bytes().unwrap();
        let decoded = Message::from_bytes(&bytes).unwrap();
        
        match decoded {
            Message::Register { local_port, remote_port } => {
                assert_eq!(local_port, 8080);
                assert_eq!(remote_port, 9000);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
