// File: lib.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     HTTP, gRPC, and P2P networking for AURIA Runtime Core.
//     Provides network server implementation for handling external requests
//     via HTTP (OpenAI-compatible API), gRPC protocols, and P2P communication.
//
pub mod p2p;

use auria_core::{AuriaResult, ExecutionOutput, ExecutionState, RequestId, RoutingDecision, ShardId, Tensor, Tier};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub struct NetworkServer {
    http_port: u16,
    grpc_port: u16,
    handlers: Arc<RwLock<Vec<Box<dyn RequestHandler>>>>,
    active_requests: Arc<RwLock<HashMap<RequestId, RequestState>>>,
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
}

pub struct InferenceRequest {
    pub tier: Tier,
    pub prompt: String,
    pub max_tokens: u32,
}

pub struct InferenceResponse {
    pub request_id: RequestId,
    pub tokens: Vec<String>,
    pub usage: UsageInfo,
}

pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
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
    peers: Arc<RwLock<Vec<String>>>,
    address: String,
}

impl P2PNode {
    pub fn new(node_id: String, address: String) -> Self {
        Self {
            node_id,
            peers: Arc::new(RwLock::new(Vec::new())),
            address,
        }
    }

    pub async fn connect(&self, peer_address: String) -> AuriaResult<()> {
        let mut peers = self.peers.write().await;
        if !peers.contains(&peer_address) {
            peers.push(peer_address);
        }
        Ok(())
    }

    pub async fn disconnect(&self, peer_address: String) -> AuriaResult<()> {
        let mut peers = self.peers.write().await;
        peers.retain(|p| p != &peer_address);
        Ok(())
    }

    pub async fn broadcast(&self, message: &[u8]) -> AuriaResult<()> {
        let peers = self.peers.read().await;
        for _peer in peers.iter() {
        }
        Ok(())
    }

    pub async fn get_peers(&self) -> Vec<String> {
        self.peers.read().await.clone()
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
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
