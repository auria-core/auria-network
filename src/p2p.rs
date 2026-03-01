// File: p2p.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     P2P networking for AURIA Runtime Core.
//     Provides peer-to-peer communication, discovery, and data exchange.

use auria_core::{AuriaError, AuriaResult, RequestId, ShardId};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};
use futures::{SinkExt, StreamExt};

const PROTOCOL_VERSION: u32 = 1;
const MAX_PEERS: usize = 50;

#[derive(Clone)]
pub struct P2PConfig {
    pub listen_address: String,
    pub listen_port: u16,
    pub bootstrap_nodes: Vec<String>,
    pub max_peers: usize,
    pub enable_discovery: bool,
}

impl Default for P2PConfig {
    fn default() -> Self {
        Self {
            listen_address: "0.0.0.0".to_string(),
            listen_port: 9000,
            bootstrap_nodes: Vec::new(),
            max_peers: MAX_PEERS,
            enable_discovery: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum P2PMessage {
    Ping { nonce: u64 },
    Pong { nonce: u64 },
    FindNode { target_id: Vec<u8>, nonce: u64 },
    Nodes { nodes: Vec<PeerInfo>, nonce: u64 },
    FindValue { key: Vec<u8>, nonce: u64 },
    Value { value: Vec<u8>, nonce: u64 },
    Store { key: Vec<u8>, value: Vec<u8> },
    GetShard { shard_id: [u8; 32] },
    ShardData { shard_id: [u8; 32], data: Vec<u8> },
    RequestInference { request_id: [u8; 16], prompt: String, max_tokens: u32 },
    InferenceResponse { request_id: [u8; 16], tokens: Vec<String> },
    Blocklist { peer_id: Vec<u8>, reason: String },
    Custom { payload: Vec<u8> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_id: Vec<u8>,
    pub address: String,
    pub port: u16,
    pub protocols: Vec<String>,
    pub version: u32,
    pub latency_ms: u64,
    pub last_seen: u64,
}

impl PeerInfo {
    pub fn new(node_id: Vec<u8>, address: String, port: u16) -> Self {
        Self {
            node_id,
            address,
            port,
            protocols: vec!["auria/1.0".to_string()],
            version: PROTOCOL_VERSION,
            latency_ms: 0,
            last_seen: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn address_string(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }
}

pub struct P2PNetwork {
    config: P2PConfig,
    node_id: Vec<u8>,
    peers: Arc<RwLock<HashMap<String, PeerConnection>>>,
    dht: Arc<RwLock<HashMap<Vec<u8>, Vec<u8>>>>,
    message_handler: Arc<RwLock<Option<Box<dyn P2PMessageHandler + Send + Sync>>>>,
    running: Arc<RwLock<bool>>,
    stats: Arc<RwLock<P2PStats>>,
}

pub struct PeerConnection {
    pub info: PeerInfo,
    pub sink: Option<futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::Message>>,
    pub connected_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct P2PStats {
    pub peer_count: usize,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub started_at: u64,
}

impl Default for P2PStats {
    fn default() -> Self {
        Self {
            peer_count: 0,
            messages_sent: 0,
            messages_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }
}

#[async_trait::async_trait]
pub trait P2PMessageHandler: Send + Sync {
    async fn handle_ping(&self, peer: &PeerInfo, nonce: u64) -> Option<P2PMessage>;
    async fn handle_find_node(&self, target_id: &[u8], nonce: u64) -> Option<P2PMessage>;
    async fn handle_store(&self, key: &[u8], value: &[u8]) -> Option<P2PMessage>;
    async fn handle_get_shard(&self, shard_id: [u8; 32]) -> Option<P2PMessage>;
    async fn handle_custom(&self, payload: &[u8]) -> Option<P2PMessage>;
}

impl P2PNetwork {
    pub fn new(config: P2PConfig) -> Self {
        let mut node_id_bytes = [0u8; 32];
        rand::thread_rng().fill(&mut node_id_bytes);
        
        Self {
            config: config.clone(),
            node_id: node_id_bytes.to_vec(),
            peers: Arc::new(RwLock::new(HashMap::new())),
            dht: Arc::new(RwLock::new(HashMap::new())),
            message_handler: Arc::new(RwLock::new(None)),
            running: Arc::new(RwLock::new(false)),
            stats: Arc::new(RwLock::new(P2PStats::default())),
        }
    }

    pub fn with_node_id(mut self, node_id: Vec<u8>) -> Self {
        self.node_id = node_id;
        self
    }

    pub fn node_id(&self) -> &[u8] {
        &self.node_id
    }

    pub async fn set_message_handler(&self, handler: Box<dyn P2PMessageHandler + Send + Sync>) {
        let mut h = self.message_handler.write().await;
        *h = Some(handler);
    }

    pub async fn start(&self) -> AuriaResult<()> {
        {
            let mut running = self.running.write().await;
            if *running {
                return Ok(());
            }
            *running = true;
        }

        tracing::info!("Starting P2P network on {}:{}", self.config.listen_address, self.config.listen_port);

        for bootstrap in &self.config.bootstrap_nodes {
            if let Err(e) = self.connect_to_peer(bootstrap).await {
                tracing::warn!("Failed to connect to bootstrap node {}: {}", bootstrap, e);
            }
        }

        Ok(())
    }

    pub async fn connect_to_peer(&self, address: &str) -> AuriaResult<()> {
        let url = if address.starts_with("ws://") || address.starts_with("wss://") {
            address.to_string()
        } else {
            format!("ws://{}", address)
        };

        let (ws_stream, _) = tokio_tungstenite::connect_async(&url).await
            .map_err(|e| AuriaError::NetworkError(format!("Failed to connect to {}: {}", address, e)))?;

        let (write, read) = ws_stream.split();

        let addr_parts: Vec<&str> = address.rsplitn(2, ':').collect();
        let port: u16 = addr_parts.first().unwrap_or(&"9000").parse().unwrap_or(9000);
        let host = addr_parts.get(1).unwrap_or(&address);

        let peer_info = PeerInfo::new(
            Self::generate_node_id(),
            host.to_string(),
            port,
        );

        let addr_key = peer_info.address_string();

        {
            let mut peers = self.peers.write().await;
            if peers.len() >= self.config.max_peers {
                return Err(AuriaError::NetworkError("Max peers reached".to_string()));
            }
            peers.insert(addr_key.clone(), PeerConnection {
                info: peer_info,
                sink: Some(write),
                connected_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            });
        }

        self.start_reader_for_peer(addr_key, read).await;

        tracing::info!("Connected to peer: {}", address);
        Ok(())
    }

    async fn start_reader_for_peer(&self, addr: String, mut read: futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>) {
        let stats = self.stats.clone();
        
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                        {
                            let mut s = stats.write().await;
                            s.messages_received += 1;
                            s.bytes_received += text.len() as u64;
                        }
                        
                        if let Ok(p2p_msg) = serde_json::from_str::<P2PMessage>(&text) {
                            tracing::debug!("Received P2P message: {:?}", p2p_msg);
                        }
                    }
                    Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                        break;
                    }
                    Err(e) => {
                        tracing::error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    fn generate_node_id() -> Vec<u8> {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill(&mut bytes);
        bytes.to_vec()
    }

    pub async fn disconnect(&self, address: &str) -> AuriaResult<()> {
        let mut peers = self.peers.write().await;
        peers.remove(address);
        Ok(())
    }

    pub async fn broadcast(&self, message: P2PMessage) -> AuriaResult<usize> {
        let data = serde_json::to_vec(&message)
            .map_err(|e| AuriaError::SerializationError(e.to_string()))?;
        
        let mut peers = self.peers.write().await;
        let mut sent = 0;
        
        for (_, conn) in peers.iter_mut() {
            if let Some(ref mut sink) = conn.sink {
                let msg = tokio_tungstenite::tungstenite::Message::Text(
                    String::from_utf8_lossy(&data).to_string()
                );
                if sink.send(msg).await.is_ok() {
                    sent += 1;
                }
            }
        }
        
        {
            let mut stats = self.stats.write().await;
            stats.messages_sent += sent as u64;
            stats.bytes_sent += (data.len() * sent) as u64;
        }
        
        Ok(sent)
    }

    pub async fn send_to(&self, address: &str, message: P2PMessage) -> AuriaResult<()> {
        let data = serde_json::to_vec(&message)
            .map_err(|e| AuriaError::SerializationError(e.to_string()))?;
        
        let mut peers = self.peers.write().await;
        
        if let Some(conn) = peers.get_mut(address) {
            if let Some(ref mut sink) = conn.sink {
                sink.send(tokio_tungstenite::tungstenite::Message::Text(
                    String::from_utf8_lossy(&data).to_string()
                )).await
                .map_err(|e| AuriaError::NetworkError(format!("Send failed: {}", e)))?;
                
                let mut stats = self.stats.write().await;
                stats.messages_sent += 1;
                stats.bytes_sent += data.len() as u64;
            }
        } else {
            return Err(AuriaError::NetworkError("Peer not found".to_string()));
        }
        
        Ok(())
    }

    pub async fn store_dht(&self, key: &[u8], value: &[u8]) -> AuriaResult<()> {
        {
            let mut dht = self.dht.write().await;
            dht.insert(key.to_vec(), value.to_vec());
        }
        
        self.broadcast(P2PMessage::Store {
            key: key.to_vec(),
            value: value.to_vec(),
        }).await?;
        
        Ok(())
    }

    pub async fn get_dht(&self, key: &[u8]) -> Option<Vec<u8>> {
        {
            let dht = self.dht.read().await;
            if let Some(value) = dht.get(key) {
                return Some(value.clone());
            }
        }
        None
    }

    pub async fn request_shard(&self, shard_id: [u8; 32]) -> AuriaResult<Vec<u8>> {
        self.broadcast(P2PMessage::GetShard { shard_id }).await?;
        
        tokio::time::sleep(Duration::from_secs(2)).await;
        
        Err(AuriaError::ShardNotFound(ShardId(shard_id)))
    }

    pub async fn get_peer_count(&self) -> usize {
        self.peers.read().await.len()
    }

    pub async fn get_peers(&self) -> Vec<PeerInfo> {
        let peers = self.peers.read().await;
        peers.values().map(|c| c.info.clone()).collect()
    }

    pub async fn get_stats(&self) -> P2PStats {
        let stats = self.stats.read().await;
        let peers = self.peers.read().await;
        
        P2PStats {
            peer_count: peers.len(),
            messages_sent: stats.messages_sent,
            messages_received: stats.messages_received,
            bytes_sent: stats.bytes_sent,
            bytes_received: stats.bytes_received,
            started_at: stats.started_at,
        }
    }

    pub async fn shutdown(&self) -> AuriaResult<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }
        
        {
            let mut peers = self.peers.write().await;
            peers.clear();
        }
        
        tracing::info!("P2P network shut down");
        
        Ok(())
    }
}

impl Default for P2PNetwork {
    fn default() -> Self {
        Self::new(P2PConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_p2p_config() {
        let config = P2PConfig::default();
        
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.max_peers, MAX_PEERS);
    }

    #[test]
    fn test_peer_info() {
        let info = PeerInfo::new(
            vec![1u8; 32],
            "127.0.0.1".to_string(),
            9000,
        );
        
        assert_eq!(info.address_string(), "127.0.0.1:9000");
    }

    #[tokio::test]
    async fn test_p2p_network_creation() {
        let network = P2PNetwork::new(P2PConfig::default());
        
        assert_eq!(network.get_peer_count().await, 0);
        assert_eq!(network.node_id().len(), 32);
    }

    #[tokio::test]
    async fn test_store_and_get_dht() {
        let network = P2PNetwork::new(P2PConfig::default());
        
        let key = b"test_key";
        let value = b"test_value";
        
        network.store_dht(key, value).await.unwrap();
        
        let retrieved = network.get_dht(key).await;
        assert!(retrieved.is_some());
    }
}
