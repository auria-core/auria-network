use auria_network::http::{HttpServer, ClusterCoordinator, ClusterConfig, WorkerNode, WorkerStatus};
use auria_network::{InferenceService, P2PNode, RequestHandler};
use auria_core::{Tier as CoreTier};
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn test_inference_service_basic() {
    let service = InferenceService::new();
    
    let request = auria_network::InferenceRequest {
        tier: CoreTier::Standard,
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
    
    for tier in [CoreTier::Nano, CoreTier::Standard, CoreTier::Pro, CoreTier::Max] {
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
        tier: CoreTier::Pro,
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
    
    assert!(supported.contains(&CoreTier::Nano));
    assert!(supported.contains(&CoreTier::Standard));
    assert!(supported.contains(&CoreTier::Pro));
    assert!(supported.contains(&CoreTier::Max));
}

#[tokio::test]
async fn test_cluster_coordinator_creation() {
    let config = ClusterConfig {
        cluster_id: "test-cluster".to_string(),
        heartbeat_interval_ms: 1000,
        election_timeout_ms: 5000,
        max_workers: 10,
        task_timeout_seconds: 300,
        failure_detection_threshold: 3,
    };
    
    let coordinator = ClusterCoordinator::with_config(config);
    
    assert!(!coordinator.node_id().is_empty());
}

#[tokio::test]
async fn test_cluster_worker_add() {
    let coordinator = ClusterCoordinator::new("test-node".to_string());
    
    let worker = WorkerNode {
        id: "worker-1".to_string(),
        address: "192.168.1.10:8080".to_string(),
        capabilities: CoreTier::Standard,
        status: WorkerStatus::Idle,
        load: 0.0,
        memory_used_mb: 0,
        memory_total_mb: 8192,
        cpu_cores: 4,
        gpu_available: true,
        started_at: 0,
        last_seen: 0,
    };
    
    let result = coordinator.add_worker(worker).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_cluster_stats() {
    let coordinator = ClusterCoordinator::new("test-node".to_string());
    
    let stats = coordinator.get_cluster_stats().await;
    
    assert_eq!(stats.total_workers, 0);
    assert_eq!(stats.pending_tasks, 0);
    assert!(!stats.is_leader);
}

#[tokio::test]
async fn test_inference_response_tokens() {
    let service = InferenceService::new();
    
    let request = auria_network::InferenceRequest {
        tier: CoreTier::Max,
        prompt: "What is the meaning of life?".to_string(),
        max_tokens: 200,
    };
    
    let response = service.handle_request(request).await.unwrap();
    
    assert!(!response.tokens.is_empty());
    let combined: String = response.tokens.join(" ");
    assert!(combined.len() > 0);
}

#[tokio::test]
async fn test_distributed_inference_basic() {
    let coordinator = ClusterCoordinator::new("test-node".to_string());
    
    let result = coordinator.execute_inference(
        "Hello distributed world".to_string(),
        50,
        CoreTier::Standard,
    ).await;
    
    assert!(result.is_ok());
    let inference = result.unwrap();
    assert!(!inference.request_id.is_empty());
    assert!(!inference.tokens.is_empty());
    assert_eq!(inference.distributed, false);
}

#[tokio::test]
async fn test_distributed_inference_all_tiers() {
    let coordinator = ClusterCoordinator::new("test-node".to_string());
    
    for tier in [CoreTier::Nano, CoreTier::Standard, CoreTier::Pro, CoreTier::Max] {
        let result = coordinator.execute_inference(
            format!("Test for {:?}", tier),
            20,
            tier,
        ).await;
        
        assert!(result.is_ok(), "Failed for tier {:?}", tier);
        let inference = result.unwrap();
        assert!(!inference.tokens.is_empty(), "Empty tokens for tier {:?}", tier);
    }
}

#[test]
fn test_ws_stream_chunk_structure() {
    let chunk = auria_network::http::WsStreamChunk {
        id: "test-id".to_string(),
        model: "test-model".to_string(),
        choices: vec![auria_network::http::WsStreamChoice {
            index: 0,
            delta: auria_network::http::WsDelta {
                content: "Hello".to_string(),
                role: Some("assistant".to_string()),
            },
            finish_reason: None,
        }],
    };
    
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("test-id"));
    assert!(json.contains("Hello"));
    assert!(json.contains("delta"));
}

#[test]
fn test_ws_stream_final_chunk() {
    let chunk = auria_network::http::WsStreamChunk {
        id: "test-id".to_string(),
        model: "test-model".to_string(),
        choices: vec![auria_network::http::WsStreamChoice {
            index: 0,
            delta: auria_network::http::WsDelta {
                content: " world".to_string(),
                role: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
    };
    
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("stop"));
}
