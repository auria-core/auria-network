use auria_network::http::HttpServer;
use auria_network::{InferenceService, P2PNode};
use auria_network::RequestHandler;
use auria_core::{Tier, RequestId};
use std::net::SocketAddr;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn test_inference_service_basic() {
    let service = InferenceService::new();
    
    let request = auria_network::InferenceRequest {
        tier: Tier::Standard,
        prompt: "Hello, world!".to_string(),
        max_tokens: 10,
    };
    
    let response = service.handle_request(request).await;
    assert!(response.is_ok());
    
    let resp = response.unwrap();
    assert!(!resp.tokens.is_empty());
}

#[tokio::test]
async fn test_inference_service_all_tiers() {
    let service = InferenceService::new();
    
    for tier in [Tier::Nano, Tier::Standard, Tier::Pro, Tier::Max] {
        let request = auria_network::InferenceRequest {
            tier,
            prompt: format!("Test for {:?}", tier),
            max_tokens: 5,
        };
        
        let response = service.handle_request(request).await;
        assert!(response.is_ok(), "Failed for tier {:?}", tier);
    }
}

#[tokio::test]
async fn test_p2p_node_creation() {
    let node = P2PNode::new(
        "test-node".to_string(),
        "127.0.0.1:9000".to_string(),
    );
    
    assert_eq!(node.node_id(), "test-node");
}

#[tokio::test]
async fn test_p2p_connect_disconnect() {
    let node = P2PNode::new(
        "test-node".to_string(),
        "127.0.0.1:9000".to_string(),
    );
    
    let result = node.connect_p2p("192.168.1.1:9000".to_string()).await;
    assert!(result.is_ok());
    
    let peers = node.get_peers().await;
    assert_eq!(peers.len(), 1);
    
    let result = node.disconnect_p2p("192.168.1.1:9000".to_string()).await;
    assert!(result.is_ok());
    
    let peers = node.get_peers().await;
    assert!(peers.is_empty());
}

#[tokio::test]
async fn test_p2p_duplicate_connect() {
    let node = P2PNode::new(
        "test-node".to_string(),
        "127.0.0.1:9000".to_string(),
    );
    
    node.connect_p2p("192.168.1.1:9000".to_string()).await.unwrap();
    node.connect_p2p("192.168.1.1:9000".to_string()).await.unwrap();
    
    let peers = node.get_peers().await;
    assert_eq!(peers.len(), 1);
}

#[tokio::test]
async fn test_inference_request_response() {
    let service = InferenceService::new();
    
    let request = auria_network::InferenceRequest {
        tier: Tier::Pro,
        prompt: "Tell me a story".to_string(),
        max_tokens: 50,
    };
    
    let response = service.handle_request(request).await.unwrap();
    
    assert!(!response.tokens.is_empty());
    assert!(response.usage.total_tokens > 0);
}

#[tokio::test]
async fn test_supported_tiers() {
    let service = InferenceService::new();
    
    let supported = service.supported_tiers();
    
    assert!(supported.contains(&Tier::Nano));
    assert!(supported.contains(&Tier::Standard));
    assert!(supported.contains(&Tier::Pro));
    assert!(supported.contains(&Tier::Max));
}
