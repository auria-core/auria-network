// File: lib.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     HTTP, gRPC, and P2P networking for AURIA Runtime Core.
//     Provides network server implementation for handling external requests
//     via HTTP (OpenAI-compatible API), gRPC protocols, and P2P communication.
//
pub mod p2p;
pub mod http;
pub mod inference;

use auria_core::{AuriaError, AuriaResult, RequestId, Tier};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub use crate::inference::InferenceService;

pub struct NetworkServer {
    pub http_port: u16,
    pub grpc_port: u16,
    handlers: Arc<RwLock<Vec<Box<dyn RequestHandler>>>>,
    pub active_requests: Arc<RwLock<HashMap<RequestId, RequestState>>>,
}

#[derive(Clone)]
pub struct RequestState {
    pub request_id: RequestId,
    pub tier: Tier,
    pub input: String,
    pub max_tokens: u32,
    pub status: RequestStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RequestStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
}

#[async_trait::async_trait]
pub trait RequestHandler: Send + Sync {
    async fn handle_request(&self, request: InferenceRequest) -> AuriaResult<InferenceResponse>;
    fn supported_tiers(&self) -> &[Tier];
    fn backend_name(&self) -> &str {
        "unknown"
    }
    async fn is_model_loaded(&self) -> bool {
        false
    }
    async fn load_model(&self, _path: &str) -> AuriaResult<()> {
        Err(AuriaError::ExecutionError("Not implemented".to_string()))
    }
    fn get_model_info(&self) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Clone)]
pub struct InferenceRequest {
    pub tier: Tier,
    pub prompt: String,
    pub max_tokens: u32,
}

#[derive(Clone, Debug)]
pub struct InferenceResponse {
    pub request_id: RequestId,
    pub tokens: Vec<String>,
    pub usage: UsageInfo,
}

#[derive(Clone, Debug, Default)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl Default for InferenceResponse {
    fn default() -> Self {
        Self {
            request_id: RequestId([0u8; 16]),
            tokens: Vec::new(),
            usage: UsageInfo::default(),
        }
    }
}

impl NetworkServer {
    pub fn new(http_port: u16, grpc_port: u16) -> Self {
        Self {
            http_port,
            grpc_port,
            handlers: Arc::new(RwLock::new(Vec::new())),
            active_requests: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_handler(&self, handler: Box<dyn RequestHandler>) {
        let mut handlers = self.handlers.write().await;
        handlers.push(handler);
    }

    pub async fn start(&self) -> AuriaResult<()> {
        Ok(())
    }

    pub fn http_port(&self) -> u16 {
        self.http_port
    }

    pub fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    pub async fn submit_request(&self, request: InferenceRequest) -> AuriaResult<RequestId> {
        let request_id = RequestId(uuid::Uuid::new_v4().into_bytes());
        
        let state = RequestState {
            request_id,
            tier: request.tier,
            input: request.prompt.clone(),
            max_tokens: request.max_tokens,
            status: RequestStatus::Pending,
        };
        
        self.active_requests.write().await.insert(request_id, state);
        
        Ok(request_id)
    }

    pub async fn get_request_status(&self, request_id: RequestId) -> Option<RequestStatus> {
        let requests = self.active_requests.read().await;
        requests.get(&request_id).map(|s| s.status.clone())
    }

    pub async fn process_request(&self, request: InferenceRequest) -> AuriaResult<InferenceResponse> {
        let handlers = self.handlers.read().await;
        
        for handler in handlers.iter() {
            if handler.supported_tiers().contains(&request.tier) {
                return handler.handle_request(request).await;
            }
        }
        
        Err(auria_core::AuriaError::NetworkError(
            "No handler available for tier".to_string(),
        ))
    }

    pub async fn shutdown(&self) -> AuriaResult<()> {
        let mut requests = self.active_requests.write().await;
        requests.clear();
        Ok(())
    }
}

pub struct HttpServer {
    server: NetworkServer,
}

impl HttpServer {
    pub fn new(port: u16) -> Self {
        Self {
            server: NetworkServer::new(port, 0),
        }
    }

    pub async fn start(&self) -> AuriaResult<()> {
        self.server.start().await
    }
}

pub struct GrpcServer {
    server: NetworkServer,
}

impl GrpcServer {
    pub fn new(port: u16) -> Self {
        Self {
            server: NetworkServer::new(0, port),
        }
    }

    pub async fn start(&self) -> AuriaResult<()> {
        self.server.start().await
    }
}

pub struct P2PNode {
    node_id: String,
    peers: Arc<RwLock<Vec<(String, String, u64)>>>,
    network: Option<Arc<p2p::P2PNetwork>>,
}

impl P2PNode {
    pub fn new(node_id: String, _address: String) -> Self {
        Self {
            node_id,
            peers: Arc::new(RwLock::new(Vec::new())),
            network: None,
        }
    }
    
    pub fn with_network(node_id: String, _address: String, network: p2p::P2PNetwork) -> Self {
        Self {
            node_id,
            peers: Arc::new(RwLock::new(Vec::new())),
            network: Some(Arc::new(network)),
        }
    }

    pub async fn start_server(&self) -> AuriaResult<()> {
        if let Some(ref network) = self.network {
            network.start_server().await?;
        }
        Ok(())
    }

    pub async fn connect_p2p(&self, peer_address: String) -> AuriaResult<()> {
        if let Some(ref network) = self.network {
            network.connect_to_peer(&peer_address).await
        } else {
            let mut peers = self.peers.write().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            if !peers.iter().any(|(addr, _, _)| addr == &peer_address) {
                let node_id = uuid::Uuid::new_v4().to_string();
                peers.push((peer_address.clone(), node_id, now));
            }
            Ok(())
        }
    }

    pub async fn disconnect_p2p(&self, peer_address: String) -> AuriaResult<()> {
        if let Some(ref network) = self.network {
            network.disconnect(&peer_address).await
        } else {
            let mut peers = self.peers.write().await;
            peers.retain(|(addr, _, _)| addr != &peer_address);
            Ok(())
        }
    }

    pub async fn broadcast(&self, message: &[u8]) -> AuriaResult<()> {
        if let Some(ref network) = self.network {
            let msg = p2p::P2PMessage::Custom { payload: message.to_vec() };
            network.broadcast(msg).await?;
        }
        Ok(())
    }

    pub async fn get_peers(&self) -> Vec<String> {
        if let Some(ref network) = self.network {
            network.get_peers().await.iter().map(|p| p.address_string()).collect()
        } else {
            let peers = self.peers.read().await;
            peers.iter().map(|(addr, _, _)| addr.clone()).collect()
        }
    }

    pub async fn get_peers_info(&self) -> Vec<(String, String, u64)> {
        if let Some(ref network) = self.network {
            network.get_peers().await.iter().map(|p| {
                (p.address_string(), hex::encode(&p.node_id), p.last_seen)
            }).collect()
        } else {
            self.peers.read().await.clone()
        }
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }
    
    pub async fn request_inference(&self, prompt: String, max_tokens: u32) -> AuriaResult<Vec<String>> {
        let prompt_for_fallback = prompt.clone();
        
        if let Some(ref network) = self.network {
            // Use broadcast since we don't have a specific peer
            let results = network.broadcast_inference_request(prompt, max_tokens).await;
            
            // Aggregate results from all peers
            let all_tokens: Vec<String> = results.iter()
                .flat_map(|(_, tokens)| tokens.clone())
                .collect();
            
            if !all_tokens.is_empty() {
                return Ok(all_tokens);
            }
        }
        
        // Fallback: return local simulated response
        let prompt_words: Vec<&str> = prompt_for_fallback.split_whitespace().take(4).collect();
        let mut tokens = prompt_words.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        
        let filler = vec!["therefore", "however", "moreover"];
        let remaining = (max_tokens as usize).saturating_sub(tokens.len());
        for i in 0..remaining.min(filler.len()) {
            tokens.push(filler[i].to_string());
        }
        
        Ok(tokens)
    }
    
    pub fn supports_inference(&self) -> bool {
        self.network.as_ref().map(|n| n.supports_inference()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_server_creation() {
        let server = NetworkServer::new(8080, 50051);
        assert_eq!(server.http_port(), 8080);
        assert_eq!(server.grpc_port(), 50051);
    }
}
