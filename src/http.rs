// File: http.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     HTTP server implementation for AURIA Runtime Core.
//     Provides OpenAI-compatible REST API for inference requests.

use axum::{
    extract::{State, Path, WebSocketUpgrade},
    http::{StatusCode},
    response::{IntoResponse, Response, Json, sse::{Event, Sse}},
    routing::{get, post},
    Router,
};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{stream, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::RwLock;

use crate::{NetworkServer, RequestStatus};
use crate::P2PNode;
use crate::{InferenceRequest, InferenceResponse, RequestHandler, UsageInfo};
use auria_core::{RequestId, Tier, UsageStats, ExpertId};
use auria_observability::MetricsCollector;
use auria_settlement::OnChainSettlement;

#[derive(Clone)]
pub struct ClusterCoordinator {
    node_id: String,
}

impl ClusterCoordinator {
    pub fn new(node_id: String) -> Self {
        Self { node_id }
    }
    
    pub fn with_config(_config: ClusterConfig) -> Self {
        let node_id = uuid::Uuid::new_v4().to_string();
        Self { node_id }
    }
    
    pub async fn init_raft(&mut self, _peers: Vec<String>) -> Result<(), String> {
        Ok(())
    }
    
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
    
    pub async fn get_cluster_stats(&self) -> ClusterStats {
        ClusterStats {
            total_workers: 0,
            idle_workers: 0,
            busy_workers: 0,
            offline_workers: 0,
            pending_tasks: 0,
            running_tasks: 0,
            completed_tasks: 0,
            failed_tasks: 0,
            is_leader: false,
            leader_id: None,
        }
    }
    
    pub async fn get_raft_info(&self) -> Option<ClusterInfo> {
        None
    }
    
    pub async fn add_worker(&self, _worker: WorkerNode) -> Result<(), String> {
        Ok(())
    }

    pub async fn execute_inference(&self, prompt: String, max_tokens: u32, tier: Tier) -> Result<DistributedResult, String> {
        let tokens = Self::simulate_inference(&prompt, max_tokens, &tier);
        
        Ok(DistributedResult {
            request_id: uuid::Uuid::new_v4().to_string(),
            tokens,
            execution_time_ms: 50,
            worker_id: self.node_id.clone(),
            distributed: false,
        })
    }

    fn simulate_inference(prompt: &str, max_tokens: u32, tier: &Tier) -> Vec<String> {
        let base_response: Vec<&str> = match tier {
            Tier::Nano => vec!["Okay", "sounds", "good", "!"],
            Tier::Standard => vec!["Here", "is", "some", "information"],
            Tier::Pro => vec!["That's", "an", "interesting", "question"],
            Tier::Max => vec!["Based", "on", "extensive", "analysis"],
        };
        
        let filler: Vec<&str> = vec![
            "however", "moreover", "therefore", "additionally", 
            "consequently", "furthermore", "hence", "thus"
        ];
        
        let mut tokens = Vec::new();
        for (i, word) in base_response.iter().enumerate() {
            if (i as u32) < max_tokens {
                tokens.push(word.to_string());
            }
        }
        
        let remaining = max_tokens as usize - tokens.len();
        for i in 0..remaining.min(filler.len()) {
            tokens.push(filler[i].to_string());
        }
        
        tokens
    }
}

#[derive(Clone, Debug)]
pub struct DistributedResult {
    pub request_id: String,
    pub tokens: Vec<String>,
    pub execution_time_ms: u64,
    pub worker_id: String,
    pub distributed: bool,
}

#[derive(Clone, Debug)]
pub struct WorkerNode {
    pub id: String,
    pub address: String,
    pub capabilities: Tier,
    pub status: WorkerStatus,
    pub load: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub cpu_cores: u32,
    pub gpu_available: bool,
    pub started_at: u64,
    pub last_seen: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkerStatus {
    Idle,
    Busy,
    Starting,
    Stopping,
    Offline,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct ClusterStats {
    pub total_workers: usize,
    pub idle_workers: usize,
    pub busy_workers: usize,
    pub offline_workers: usize,
    pub pending_tasks: usize,
    pub running_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub is_leader: bool,
    pub leader_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClusterInfo {
    pub role: NodeRole,
    pub term: u64,
    pub commit_index: u64,
    pub log_length: usize,
    pub peers: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum NodeRole {
    Follower,
    Candidate,
    Leader,
}

pub struct ClusterConfig {
    pub cluster_id: String,
    pub heartbeat_interval_ms: u64,
    pub election_timeout_ms: u64,
    pub max_workers: usize,
    pub task_timeout_seconds: u64,
    pub failure_detection_threshold: u32,
}

#[derive(Clone)]
pub struct HttpServerState {
    pub network_server: Arc<NetworkServer>,
    pub p2p_node: Arc<RwLock<Option<P2PNode>>>,
    pub inference_handlers: Arc<RwLock<Vec<Box<dyn RequestHandler>>>>,
    pub metrics: Arc<MetricsCollector>,
    pub settlement: Arc<RwLock<Option<OnChainSettlement>>>,
    pub cluster: Arc<RwLock<Option<ClusterCoordinator>>>,
}

impl HttpServerState {
    pub fn new(network_server: NetworkServer) -> Self {
        Self {
            network_server: Arc::new(network_server),
            p2p_node: Arc::new(RwLock::new(None)),
            inference_handlers: Arc::new(RwLock::new(Vec::new())),
            metrics: Arc::new(MetricsCollector::new()),
            settlement: Arc::new(RwLock::new(None)),
            cluster: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn register_inference_handler(&self, handler: Box<dyn RequestHandler>) {
        let mut handlers = self.inference_handlers.write().await;
        handlers.push(handler);
    }

    pub async fn set_p2p_node(&self, node: P2PNode) {
        let mut p2p = self.p2p_node.write().await;
        *p2p = Some(node);
    }

    pub async fn set_settlement(&self, settlement: OnChainSettlement) {
        let mut s = self.settlement.write().await;
        *s = Some(settlement);
    }

    pub async fn set_cluster(&self, cluster: ClusterCoordinator) {
        let mut c = self.cluster.write().await;
        *c = Some(cluster);
    }

    pub async fn add_settlement_receipt(
        &self,
        request_id: RequestId,
        expert_ids: Vec<ExpertId>,
        usage: UsageStats,
    ) -> Option<String> {
        let s = self.settlement.read().await;
        if let Some(settlement) = s.as_ref() {
            settlement.add_receipt(request_id, expert_ids, usage).await.ok()
        } else {
            None
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionChoice {
    pub text: String,
    pub index: u32,
    pub finish_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<Model>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub object: String,
    pub created: u32,
    pub owned_by: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeStatusResponse {
    pub node_id: String,
    pub peers: Vec<String>,
    pub active_requests: usize,
    pub p2p_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitRequestResponse {
    pub request_id: String,
    pub status: String,
}

fn tier_from_string(tier_str: &str) -> Tier {
    match tier_str.to_lowercase().as_str() {
        "nano" => Tier::Nano,
        "standard" => Tier::Standard,
        "pro" => Tier::Pro,
        "max" => Tier::Max,
        _ => Tier::Standard,
    }
}

async fn chat_completions(
    State(state): State<HttpServerState>,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, AppError> {
    let start_time = std::time::Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let messages_str: Vec<String> = request.messages
        .iter()
        .map(|m| format!("{:?}: {}", m.role, m.content))
        .collect();
    let prompt = messages_str.join("\n");

    let tier = tier_from_string(&request.model);
    let max_tokens = request.max_tokens.unwrap_or(100);
    let inference_req = InferenceRequest {
        tier,
        prompt: prompt.clone(),
        max_tokens,
    };

    // Try cluster coordinator first for distributed inference
    let cluster_guard = state.cluster.read().await;
    let mut response_text = String::new();
    let mut success = false;
    
    if let Some(ref cluster) = *cluster_guard {
        // Check if cluster is the leader and has workers available
        let stats = cluster.get_cluster_stats().await;
        if stats.is_leader && stats.total_workers > 0 {
            match cluster.execute_inference(prompt.clone(), max_tokens, tier.clone()).await {
                Ok(result) => {
                    tracing::info!("Distributed inference completed: {} tokens in {}ms", 
                        result.tokens.len(), result.execution_time_ms);
                    response_text = result.tokens.join(" ");
                    success = true;
                }
                Err(e) => {
                    tracing::warn!("Cluster inference failed, falling back to handlers: {}", e);
                }
            }
        }
    }
    
    drop(cluster_guard);

    // Fall back to inference handlers if cluster didn't produce results
    if response_text.is_empty() {
        let handlers = state.inference_handlers.read().await;
        
        for handler in handlers.iter() {
            if handler.supported_tiers().contains(&inference_req.tier) {
                match handler.handle_request(inference_req.clone()).await {
                    Ok(resp) => {
                        response_text = resp.tokens.join(" ");
                        success = true;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("Handler error: {:?}", e);
                    }
                }
            }
        }

        if response_text.is_empty() {
            response_text = format!("Simulated response for: {}", request.messages.last().map(|m| m.content.as_str()).unwrap_or(""));
        }
    }

    let latency_ms = start_time.elapsed().as_secs_f64() * 1000.0;
    let tier_name = format!("{:?}", tier).to_lowercase();
    let mut labels = std::collections::HashMap::new();
    labels.insert("tier".to_string(), tier_name.clone());
    labels.insert("type".to_string(), "chat".to_string());
    state.metrics.increment_counter("auria_requests_total", 1, labels.clone()).await;
    state.metrics.record_histogram("auria_request_latency_ms", latency_ms).await;
    if success {
        state.metrics.increment_counter("auria_requests_success", 1, labels).await;
    } else {
        let mut error_labels = labels.clone();
        error_labels.insert("error".to_string(), "no_handler".to_string());
        state.metrics.increment_counter("auria_requests_failed", 1, error_labels).await;
    }

    let response_len = response_text.len();
    let prompt_tokens = (request.messages.iter().map(|m| m.content.len()).sum::<usize>() / 4) as u32;
    let completion_tokens = (response_len / 4) as u32;
    let total_tokens = ((request.messages.iter().map(|m| m.content.len()).sum::<usize>() + response_len) / 4) as u32;
    
    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", request_id),
        object: "chat.completion".to_string(),
        created,
        model: request.model.clone(),
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: response_text,
            },
            finish_reason: "stop".to_string(),
        }],
        usage: Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        },
    };
    
    // Generate settlement receipt if settlement is configured
    if success {
        let request_id_bytes: [u8; 16] = uuid::Uuid::new_v4().into_bytes();
        let core_request_id = RequestId(request_id_bytes);
        let usage_stats = UsageStats {
            tokens_generated: completion_tokens as u64,
            tokens_processed: total_tokens as u64,
        };
        
        let _ = state.add_settlement_receipt(
            core_request_id,
            vec![], // Empty expert IDs - would be populated by actual expert routing
            usage_stats,
        ).await;
    }

    Ok(Json(response))
}

async fn completions(
    State(state): State<HttpServerState>,
    Json(request): Json<CompletionRequest>,
) -> Result<Json<CompletionResponse>, AppError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let inference_req = InferenceRequest {
        tier: tier_from_string(&request.model),
        prompt: request.prompt.clone(),
        max_tokens: request.max_tokens.unwrap_or(100),
    };

    let handlers = state.inference_handlers.read().await;
    let mut response_text = String::new();
    
    for handler in handlers.iter() {
        if handler.supported_tiers().contains(&inference_req.tier) {
            match handler.handle_request(inference_req.clone()).await {
                Ok(resp) => {
                    response_text = resp.tokens.join(" ");
                    break;
                }
                Err(e) => {
                    tracing::warn!("Handler error: {:?}", e);
                }
            }
        }
    }

    if response_text.is_empty() {
        response_text = format!("Simulated completion for: {}", request.prompt);
    }

    let response_len = response_text.len();
    let response = CompletionResponse {
        id: format!("cmpl-{}", request_id),
        object: "text_completion".to_string(),
        created,
        model: request.model.clone(),
        choices: vec![CompletionChoice {
            text: response_text,
            index: 0,
            finish_reason: "stop".to_string(),
        }],
        usage: Usage {
            prompt_tokens: (request.prompt.len() / 4) as u32,
            completion_tokens: (response_len / 4) as u32,
            total_tokens: ((request.prompt.len() + response_len) / 4) as u32,
        },
    };

    Ok(Json(response))
}

async fn list_models() -> Json<ModelsResponse> {
    Json(ModelsResponse {
        object: "list".to_string(),
        data: vec![
            Model {
                id: "auria-nano".to_string(),
                object: "model".to_string(),
                created: 1700000000,
                owned_by: "auria".to_string(),
            },
            Model {
                id: "auria-standard".to_string(),
                object: "model".to_string(),
                created: 1700000000,
                owned_by: "auria".to_string(),
            },
            Model {
                id: "auria-pro".to_string(),
                object: "model".to_string(),
                created: 1700000000,
                owned_by: "auria".to_string(),
            },
            Model {
                id: "auria-max".to_string(),
                object: "model".to_string(),
                created: 1700000000,
                owned_by: "auria".to_string(),
            },
        ],
    })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn node_status(
    State(state): State<HttpServerState>,
) -> Json<NodeStatusResponse> {
    let p2p_node = state.p2p_node.read().await;
    let peers: Vec<String> = if let Some(ref node) = *p2p_node {
        node.get_peers().await
    } else {
        vec![]
    };

    let active_count = {
        let requests = state.network_server.active_requests.read().await;
        requests.len()
    };

    let node_id: String = if let Some(ref node) = *p2p_node {
        node.node_id().to_string()
    } else {
        "unconfigured".to_string()
    };

    Json(NodeStatusResponse {
        node_id,
        peers,
        active_requests: active_count,
        p2p_enabled: p2p_node.is_some(),
    })
}

async fn get_metrics(
    State(state): State<HttpServerState>,
) -> String {
    let mut metrics_output = state.metrics.get_all_metrics().await;
    
    metrics_output.push_str("# HELP auria_active_requests Current number of active requests\n");
    metrics_output.push_str("# TYPE auria_active_requests gauge\n");
    let active = {
        let requests = state.network_server.active_requests.read().await;
        requests.len() as f64
    };
    metrics_output.push_str(&format!("auria_active_requests {}\n\n", active));
    
    metrics_output.push_str("# HELP auria_uptime_seconds Node uptime in seconds\n");
    metrics_output.push_str("# TYPE auria_uptime_seconds counter\n");
    metrics_output.push_str(&format!("auria_uptime_seconds {}\n", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()));
    
    metrics_output
}

#[derive(Debug, Deserialize)]
pub struct ConnectPeerRequest {
    pub address: String,
    #[serde(default)]
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct PeerListResponse {
    pub peers: Vec<PeerInfo>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct PeerActionResponse {
    pub success: bool,
    pub message: String,
    pub peer: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    pub node_id: String,
    pub address: String,
    pub connected_at: u64,
    pub latency_ms: u64,
}

async fn list_peers(
    State(state): State<HttpServerState>,
) -> Json<PeerListResponse> {
    let p2p = state.p2p_node.read().await;
    let peers = if let Some(ref node) = *p2p {
        node.get_peers_info().await
    } else {
        Vec::new()
    };
    
    let peer_infos: Vec<PeerInfo> = peers.into_iter().map(|(id, addr, time)| {
        PeerInfo {
            node_id: hex::encode(&id),
            address: addr,
            connected_at: time,
            latency_ms: 0,
        }
    }).collect();

    Json(PeerListResponse {
        count: peer_infos.len(),
        peers: peer_infos,
    })
}

async fn connect_peer(
    State(state): State<HttpServerState>,
    Json(request): Json<ConnectPeerRequest>,
) -> Json<PeerActionResponse> {
    let p2p = state.p2p_node.read().await;
    
    if let Some(ref node) = *p2p {
        let address = if request.port > 0 {
            format!("{}:{}", request.address, request.port)
        } else {
            request.address.clone()
        };
        
        match node.connect_p2p(address.clone()).await {
            Ok(()) => {
                Json(PeerActionResponse {
                    success: true,
                    message: "Connected to peer".to_string(),
                    peer: Some(request.address),
                })
            }
            Err(e) => {
                Json(PeerActionResponse {
                    success: false,
                    message: e.to_string(),
                    peer: Some(request.address),
                })
            }
        }
    } else {
        Json(PeerActionResponse {
            success: false,
            message: "P2P not initialized".to_string(),
            peer: None,
        })
    }
}

async fn disconnect_peer(
    State(state): State<HttpServerState>,
    Json(request): Json<ConnectPeerRequest>,
) -> Json<PeerActionResponse> {
    let p2p = state.p2p_node.read().await;
    
    if let Some(ref node) = *p2p {
        let address = if request.port > 0 {
            format!("{}:{}", request.address, request.port)
        } else {
            request.address.clone()
        };
        
        match node.disconnect_p2p(address.clone()).await {
            Ok(()) => {
                Json(PeerActionResponse {
                    success: true,
                    message: "Disconnected from peer".to_string(),
                    peer: Some(request.address),
                })
            }
            Err(e) => {
                Json(PeerActionResponse {
                    success: false,
                    message: e.to_string(),
                    peer: Some(request.address),
                })
            }
        }
    } else {
        Json(PeerActionResponse {
            success: false,
            message: "P2P not initialized".to_string(),
            peer: None,
        })
    }
}

async fn submit_request(
    State(state): State<HttpServerState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<SubmitRequestResponse>, AppError> {
    let tier_str = request.get("tier")
        .and_then(|t| t.as_str())
        .unwrap_or("standard");
    let prompt = request.get("prompt")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    let max_tokens = request.get("max_tokens")
        .and_then(|m| m.as_u64())
        .map(|m| m as u32)
        .unwrap_or(100);

    let inference_req = InferenceRequest {
        tier: tier_from_string(tier_str),
        prompt,
        max_tokens,
    };

    let request_id = state.network_server.submit_request(inference_req).await
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(Json(SubmitRequestResponse {
        request_id: hex::encode(request_id.0),
        status: "submitted".to_string(),
    }))
}

async fn get_request_status(
    State(state): State<HttpServerState>,
    Path(request_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let request_id_bytes = hex::decode(&request_id)
        .map_err(|_| AppError::invalid_request("Invalid request ID".to_string()))?;
    
    let request_id = if request_id_bytes.len() == 16 {
        RequestId(request_id_bytes.try_into().unwrap_or([0u8; 16]))
    } else {
        return Err(AppError::invalid_request("Invalid request ID length".to_string()));
    };

    let status = state.network_server.get_request_status(request_id).await
        .ok_or_else(|| AppError::not_found("Request not found".to_string()))?;

    let status_str = match status {
        RequestStatus::Pending => "pending",
        RequestStatus::Running => "running",
        RequestStatus::Completed => "completed",
        RequestStatus::Failed(e) => return Ok(Json(serde_json::json!({
            "status": "failed",
            "error": e
        }))),
    };

    Ok(Json(serde_json::json!({
        "request_id": request_id,
        "status": status_str
    })))
}

#[derive(Debug)]
struct AppError {
    message: String,
    status: StatusCode,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    fn cluster_error(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = Json(ErrorResponse {
            error: ErrorDetail {
                message: self.message,
                error_type: "invalid_request_error".to_string(),
                code: None,
            },
        });

        (self.status, body).into_response()
    }
}

pub struct HttpServer {
    state: HttpServerState,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HttpServer {
    pub fn new(port: u16) -> Self {
        let network_server = NetworkServer::new(port, 0);
        Self {
            state: HttpServerState::new(network_server),
            shutdown_tx: None,
        }
    }

    pub fn state(&self) -> &HttpServerState {
        &self.state
    }

    pub async fn start(mut self, bind_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        
        let state = self.state.clone();
        let app = Router::new()
            .route("/v1/chat/completions", post(chat_completions))
            .route("/v1/chat/completions/stream", post(chat_completions_stream))
            .route("/v1/completions", post(completions))
            .route("/v1/completions/stream", post(completions_stream))
            .route("/v1/models", get(list_models))
            .route("/health", get(health))
            .route("/api/v1/status", get(node_status))
            .route("/api/v1/submit", post(submit_request))
            .route("/api/v1/status/:request_id", get(get_request_status))
            .route("/api/v1/peers", get(list_peers))
            .route("/api/v1/peers/connect", post(connect_peer))
            .route("/api/v1/peers/disconnect", post(disconnect_peer))
            .route("/api/v1/settlement/status", get(get_settlement_status))
            .route("/api/v1/settlement/submit", post(submit_settlement))
            .route("/api/v1/settlement/withdraw", post(withdraw_settlement_rewards))
            .route("/api/v1/settlement/history", get(get_settlement_history))
            .route("/api/v1/cluster/status", get(get_cluster_status))
            .route("/api/v1/cluster/workers", get(get_cluster_workers))
            .route("/api/v1/cluster/workers/add", post(add_cluster_worker))
            .route("/api/v1/cluster/distributed-infer", post(distributed_inference))
            .route("/api/v1/model/status", get(get_model_status))
            .route("/api/v1/model/load", post(load_model))
            .route("/metrics", get(get_metrics))
            .route("/ws/inference", get(websocket_inference))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        tracing::info!("HTTP server listening on {}", bind_addr);

        let server = axum::serve(listener, app);
        
        let handle = tokio::spawn(async move {
            if let Err(e) = server.with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            }).await {
                tracing::error!("HTTP server error: {}", e);
            }
        });

        self.shutdown_tx = Some(shutdown_tx);
        
        handle.await.map_err(|e| format!("Server task joined with error: {}", e))?;
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn chat_completions_stream(
    State(state): State<HttpServerState>,
    Json(request): Json<ChatCompletionRequest>,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let messages_str: Vec<String> = request.messages
        .iter()
        .map(|m| format!("{:?}: {}", m.role, m.content))
        .collect();
    let prompt = messages_str.join("\n");

    let inference_req = InferenceRequest {
        tier: tier_from_string(&request.model),
        prompt,
        max_tokens: request.max_tokens.unwrap_or(100),
    };

    let handlers = state.inference_handlers.read().await;
    
    let all_tokens: Vec<String> = if request.stream.unwrap_or(false) {
        let mut tokens = Vec::new();
        for handler in handlers.iter() {
            if handler.supported_tiers().contains(&inference_req.tier) {
                match handler.handle_request(inference_req.clone()).await {
                    Ok(resp) => {
                        tokens = resp.tokens;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("Handler error: {:?}", e);
                    }
                }
            }
        }
        if tokens.is_empty() {
            vec![format!("Response for: {}", request.messages.last().map(|m| m.content.as_str()).unwrap_or(""))]
        } else {
            tokens
        }
    } else {
        vec![]
    };

    drop(handlers);

    let model = request.model.clone();
    let completion_id = format!("chatcmpl-{}", request_id);
    let total = all_tokens.len();

    // Generate SSE events with proper event format
    let events: Vec<Result<Event, std::convert::Infallible>> = all_tokens.into_iter().enumerate()
        .map(|(i, word)| {
            let chunk = ChatCompletionChunk {
                id: completion_id.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.clone(),
                choices: vec![ChatCompletionChunkChoice {
                    index: 0,
                    delta: ChatMessageDelta {
                        role: if i == 0 { Some(MessageRole::Assistant) } else { None },
                        content: Some(word),
                    },
                    finish_reason: if i == total.saturating_sub(1) { Some("stop".to_string()) } else { None },
                }],
            };
            let data = serde_json::to_string(&chunk).unwrap_or_default();
            
            // Create SSE event with explicit event type
            let mut event = Event::default();
            event = event.data(data);
            event = event.event("message".to_string());
            
            Ok::<_, std::convert::Infallible>(event)
        })
        .collect();

    // Add [DONE] event at the end to signal completion
    let done_events = std::iter::once(Ok::<_, std::convert::Infallible>(
        Event::default()
            .data("[DONE]".to_string())
            .event("done".to_string())
    ));

    let combined = events.into_iter().chain(done_events);
    Sse::new(stream::iter(combined))
        .into_response()
}

async fn completions_stream(
    State(state): State<HttpServerState>,
    Json(request): Json<CompletionRequest>,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let inference_req = InferenceRequest {
        tier: tier_from_string(&request.model),
        prompt: request.prompt.clone(),
        max_tokens: request.max_tokens.unwrap_or(100),
    };

    let handlers = state.inference_handlers.read().await;
    
    let all_tokens: Vec<String> = if request.stream.unwrap_or(false) {
        let mut tokens = Vec::new();
        for handler in handlers.iter() {
            if handler.supported_tiers().contains(&inference_req.tier) {
                match handler.handle_request(inference_req.clone()).await {
                    Ok(resp) => {
                        tokens = resp.tokens;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("Handler error: {:?}", e);
                    }
                }
            }
        }
        if tokens.is_empty() {
            vec![format!("Completion for: {}", request.prompt)]
        } else {
            tokens
        }
    } else {
        vec![]
    };

    drop(handlers);

    let model = request.model.clone();
    let completion_id = format!("cmpl-{}", request_id);
    let total = all_tokens.len();

    // Generate SSE events with proper event format
    let events: Vec<Result<Event, std::convert::Infallible>> = all_tokens.into_iter()
        .enumerate()
        .map(|(i, text)| {
            let chunk = CompletionChunk {
                id: completion_id.clone(),
                object: "text_completion.chunk".to_string(),
                created,
                model: model.clone(),
                choices: vec![CompletionChunkChoice {
                    text: text.clone(),
                    index: 0,
                    logprobs: None,
                    finish_reason: if i == total.saturating_sub(1) { Some("stop".to_string()) } else { None },
                }],
            };
            let data = serde_json::to_string(&chunk).unwrap_or_default();
            
            // Create SSE event with explicit event type
            let mut event = Event::default();
            event = event.data(data);
            event = event.event("message".to_string());
            
            Ok::<_, std::convert::Infallible>(event)
        })
        .collect();

    // Add [DONE] event at the end to signal completion
    let done_events = std::iter::once(Ok::<_, std::convert::Infallible>(
        Event::default()
            .data("[DONE]".to_string())
            .event("done".to_string())
    ));

    let combined = events.into_iter().chain(done_events);
    Sse::new(stream::iter(combined))
        .into_response()
}

#[derive(Debug, Serialize)]
struct ChatCompletionChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionChunkChoice {
    index: u32,
    delta: ChatMessageDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatMessageDelta {
    role: Option<MessageRole>,
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct CompletionChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<CompletionChunkChoice>,
}

#[derive(Debug, Serialize)]
struct CompletionChunkChoice {
    text: String,
    index: u32,
    logprobs: Option<()>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsInferenceRequest {
    pub id: String,
    pub method: String,
    pub params: WsInferenceParams,
}

#[derive(Debug, Deserialize)]
pub struct WsInferenceParams {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct WsResponse {
    pub id: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<WsResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WsError>,
}

#[derive(Debug, Serialize)]
pub struct WsResult {
    pub model: String,
    pub choices: Vec<WsChoice>,
    pub usage: Usage,
    pub created: u64,
}

#[derive(Debug, Serialize)]
pub struct WsChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct WsError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct WsStreamChunk {
    pub id: String,
    pub model: String,
    pub choices: Vec<WsStreamChoice>,
}

#[derive(Debug, Serialize)]
pub struct WsStreamChoice {
    pub index: u32,
    pub delta: WsDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WsDelta {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SettlementStatusResponse {
    pub connected: bool,
    pub chain_id: u64,
    pub wallet_address: String,
    pub contract_address: String,
    pub pending_receipts: u32,
    pub total_settled: u64,
    pub pending_rewards: u64,
}

#[derive(Debug, Serialize)]
pub struct SettlementSubmitResponse {
    pub success: bool,
    pub tx_hash: Option<String>,
    pub message: String,
    pub receipt_count: u32,
}

#[derive(Debug, Serialize)]
pub struct SettlementHistoryResponse {
    pub submissions: Vec<SettlementHistoryItem>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct SettlementHistoryItem {
    pub submission_id: String,
    pub tx_hash: String,
    pub receipt_count: u32,
    pub merkle_root: String,
    pub status: String,
    pub submitted_at: u64,
    pub confirmed_at: Option<u64>,
    pub gas_used: Option<u64>,
}

async fn get_settlement_status(
    State(state): State<HttpServerState>,
) -> Json<SettlementStatusResponse> {
    let s = state.settlement.read().await;
    
    if let Some(settlement) = s.as_ref() {
        match settlement.get_status().await {
            Ok(status) => {
                return Json(SettlementStatusResponse {
                    connected: status.is_connected,
                    chain_id: status.chain_id,
                    wallet_address: status.wallet_address,
                    contract_address: status.contract_address,
                    pending_receipts: status.pending_receipts,
                    total_settled: status.total_settled,
                    pending_rewards: status.pending_rewards,
                });
            }
            Err(e) => {
                tracing::warn!("Failed to get settlement status: {}", e);
            }
        }
    }
    
    Json(SettlementStatusResponse {
        connected: false,
        chain_id: 0,
        wallet_address: String::new(),
        contract_address: String::new(),
        pending_receipts: 0,
        total_settled: 0,
        pending_rewards: 0,
    })
}

async fn submit_settlement(
    State(state): State<HttpServerState>,
) -> Json<SettlementSubmitResponse> {
    let s = state.settlement.read().await;
    
    if let Some(settlement) = s.as_ref() {
        match settlement.trigger_settlement().await {
            Ok(tx_hash) => {
                let pending = settlement.get_pending_receipt_count().await;
                return Json(SettlementSubmitResponse {
                    success: true,
                    tx_hash: Some(tx_hash),
                    message: "Settlement submitted successfully".to_string(),
                    receipt_count: pending as u32,
                });
            }
            Err(e) => {
                return Json(SettlementSubmitResponse {
                    success: false,
                    tx_hash: None,
                    message: e.to_string(),
                    receipt_count: 0,
                });
            }
        }
    }
    
    Json(SettlementSubmitResponse {
        success: false,
        tx_hash: None,
        message: "Settlement not configured".to_string(),
        receipt_count: 0,
    })
}

async fn withdraw_settlement_rewards(
    State(state): State<HttpServerState>,
) -> Json<SettlementSubmitResponse> {
    let s = state.settlement.read().await;
    
    if let Some(settlement) = s.as_ref() {
        match settlement.withdraw_rewards().await {
            Ok(tx_hash) => {
                return Json(SettlementSubmitResponse {
                    success: true,
                    tx_hash: Some(tx_hash),
                    message: "Rewards withdrawn successfully".to_string(),
                    receipt_count: 0,
                });
            }
            Err(e) => {
                return Json(SettlementSubmitResponse {
                    success: false,
                    tx_hash: None,
                    message: e.to_string(),
                    receipt_count: 0,
                });
            }
        }
    }
    
    Json(SettlementSubmitResponse {
        success: false,
        tx_hash: None,
        message: "Settlement not configured".to_string(),
        receipt_count: 0,
    })
}

async fn get_settlement_history(
    State(state): State<HttpServerState>,
) -> Json<SettlementHistoryResponse> {
    let s = state.settlement.read().await;
    
    if let Some(settlement) = s.as_ref() {
        let submissions = settlement.get_submission_history().await;
        let items: Vec<SettlementHistoryItem> = submissions.into_iter().map(|sub| {
            let status_str = match &sub.status {
                auria_settlement::SettlementSubmissionStatus::Pending => "pending",
                auria_settlement::SettlementSubmissionStatus::Submitted => "submitted",
                auria_settlement::SettlementSubmissionStatus::Confirmed => "confirmed",
                auria_settlement::SettlementSubmissionStatus::Failed(_) => "failed",
            };
            SettlementHistoryItem {
                submission_id: sub.submission_id,
                tx_hash: sub.tx_hash,
                receipt_count: sub.receipt_count,
                merkle_root: sub.merkle_root,
                status: status_str.to_string(),
                submitted_at: sub.submitted_at,
                confirmed_at: sub.confirmed_at,
                gas_used: sub.gas_used,
            }
        }).collect();
        
        return Json(SettlementHistoryResponse {
            total: items.len(),
            submissions: items,
        });
    }
    
    Json(SettlementHistoryResponse {
        total: 0,
        submissions: vec![],
    })
}

#[derive(Debug, Serialize)]
pub struct ClusterStatusResponse {
    pub node_id: String,
    pub is_leader: bool,
    pub leader_id: Option<String>,
    pub total_workers: usize,
    pub pending_tasks: usize,
    pub raft_info: Option<ClusterRaftInfo>,
}

#[derive(Debug, Serialize)]
pub struct ClusterRaftInfo {
    pub role: String,
    pub term: u64,
    pub commit_index: u64,
    pub log_length: usize,
    pub peers: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddWorkerRequest {
    pub id: String,
    pub address: String,
    pub capabilities: String,
    pub memory_total_mb: u64,
    pub cpu_cores: u32,
    pub gpu_available: bool,
}

#[derive(Debug, Deserialize)]
pub struct DistributedInferenceRequest {
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub tier: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DistributedInferenceResponse {
    pub request_id: String,
    pub tokens: Vec<String>,
    pub execution_time_ms: u64,
    pub worker_id: String,
    pub distributed: bool,
}

async fn get_cluster_status(
    State(state): State<HttpServerState>,
) -> Json<ClusterStatusResponse> {
    let c: tokio::sync::RwLockReadGuard<'_, Option<ClusterCoordinator>> = state.cluster.read().await;
    
    if let Some(cluster) = c.as_ref() {
        let stats = cluster.get_cluster_stats().await;
        let raft_info = cluster.get_raft_info().await.map(|r| ClusterRaftInfo {
            role: format!("{:?}", r.role),
            term: r.term,
            commit_index: r.commit_index,
            log_length: r.log_length,
            peers: r.peers,
        });
        
        return Json(ClusterStatusResponse {
            node_id: cluster.node_id().to_string(),
            is_leader: stats.is_leader,
            leader_id: stats.leader_id,
            total_workers: stats.total_workers,
            pending_tasks: stats.pending_tasks,
            raft_info,
        });
    }
    
    Json(ClusterStatusResponse {
        node_id: String::new(),
        is_leader: false,
        leader_id: None,
        total_workers: 0,
        pending_tasks: 0,
        raft_info: None,
    })
}

async fn add_cluster_worker(
    State(state): State<HttpServerState>,
    Json(request): Json<AddWorkerRequest>,
) -> Json<serde_json::Value> {
    let c: tokio::sync::RwLockReadGuard<'_, Option<ClusterCoordinator>> = state.cluster.read().await;
    
    if let Some(cluster) = c.as_ref() {
        let tier = match request.capabilities.to_lowercase().as_str() {
            "nano" => Tier::Nano,
            "standard" => Tier::Standard,
            "pro" => Tier::Pro,
            "max" => Tier::Max,
            _ => Tier::Standard,
        };
        
        let worker = WorkerNode {
            id: request.id.clone(),
            address: request.address.clone(),
            capabilities: tier,
            status: WorkerStatus::Idle,
            load: 0.0,
            memory_used_mb: 0,
            memory_total_mb: request.memory_total_mb,
            cpu_cores: request.cpu_cores,
            gpu_available: request.gpu_available,
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            last_seen: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        
        match cluster.add_worker(worker).await {
            Ok(()) => {
                return Json(serde_json::json!({
                    "success": true,
                    "message": format!("Worker {} added", request.id)
                }));
            }
            Err(e) => {
                return Json(serde_json::json!({
                    "success": false,
                    "message": e.to_string()
                }));
            }
        }
    }
    
    Json(serde_json::json!({
        "success": false,
        "message": "Cluster not initialized"
    }))
}

async fn distributed_inference(
    State(state): State<HttpServerState>,
    Json(request): Json<DistributedInferenceRequest>,
) -> Result<Json<DistributedInferenceResponse>, AppError> {
    let c: tokio::sync::RwLockReadGuard<'_, Option<ClusterCoordinator>> = state.cluster.read().await;
    
    if let Some(cluster) = c.as_ref() {
        let tier = match request.tier.as_deref() {
            Some("nano") => Tier::Nano,
            Some("standard") => Tier::Standard,
            Some("pro") => Tier::Pro,
            Some("max") => Tier::Max,
            Some(t) => {
                tracing::warn!("Unknown tier {}, defaulting to Standard", t);
                Tier::Standard
            }
            None => Tier::Standard,
        };
        
        let max_tokens = request.max_tokens.unwrap_or(100);
        
        match cluster.execute_inference(request.prompt, max_tokens, tier).await {
            Ok(result) => {
                let request_id = format!("{:?}", result.request_id);
                return Ok(Json(DistributedInferenceResponse {
                    request_id,
                    tokens: result.tokens,
                    execution_time_ms: result.execution_time_ms,
                    worker_id: result.worker_id,
                    distributed: result.distributed,
                }));
            }
            Err(e) => {
                tracing::error!("Distributed inference failed: {}", e);
                return Err(AppError::cluster_error(e));
            }
        }
    }
    
    Err(AppError::cluster_error("Cluster not initialized".to_string()))
}

async fn get_cluster_workers(
    State(state): State<HttpServerState>,
) -> Json<serde_json::Value> {
    let c: tokio::sync::RwLockReadGuard<'_, Option<ClusterCoordinator>> = state.cluster.read().await;
    
    if let Some(cluster) = c.as_ref() {
        let stats = cluster.get_cluster_stats().await;
        return Json(serde_json::json!({
            "total_workers": stats.total_workers,
            "idle_workers": stats.idle_workers,
            "busy_workers": stats.busy_workers,
            "offline_workers": stats.offline_workers,
            "is_leader": stats.is_leader,
            "leader_id": stats.leader_id,
        }));
    }
    
    Json(serde_json::json!({
        "total_workers": 0,
        "idle_workers": 0,
        "busy_workers": 0,
        "offline_workers": 0,
        "is_leader": false,
        "leader_id": null,
    }))
}

#[derive(Debug, Serialize)]
pub struct ModelStatusResponse {
    pub loaded: bool,
    pub model_path: Option<String>,
    pub model_type: Option<String>,
    pub vocab_size: Option<usize>,
    pub hidden_size: Option<usize>,
    pub num_layers: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct LoadModelRequest {
    pub model_path: String,
}

async fn get_model_status(
    State(state): State<HttpServerState>,
) -> Json<ModelStatusResponse> {
    let handlers = state.inference_handlers.read().await;
    
    let mut loaded = false;
    let mut model_path = None;
    let mut model_type = None;
    let mut vocab_size = None;
    let mut hidden_size = None;
    let mut num_layers = None;
    
    for handler in handlers.iter() {
        let handler_name = handler.backend_name();
        if handler_name.contains("cpu") || handler_name.contains("gpu") {
            loaded = handler.is_model_loaded().await;
            if let Some(info) = handler.get_model_info() {
                if let Some(path) = info.get("model_path").and_then(|v| v.as_str()) {
                    model_path = Some(path.to_string());
                }
                if let Some(mtype) = info.get("model_type").and_then(|v| v.as_str()) {
                    model_type = Some(mtype.to_string());
                }
            }
        }
    }
    
    Json(ModelStatusResponse {
        loaded,
        model_path,
        model_type,
        vocab_size,
        hidden_size,
        num_layers,
    })
}

async fn load_model(
    State(state): State<HttpServerState>,
    Json(request): Json<LoadModelRequest>,
) -> Json<serde_json::Value> {
    let handlers = state.inference_handlers.read().await;
    
    for handler in handlers.iter() {
        let handler_name = handler.backend_name();
        if handler_name.contains("cpu") || handler_name.contains("gpu") {
            match handler.load_model(&request.model_path).await {
                Ok(()) => {
                    return Json(serde_json::json!({
                        "success": true,
                        "message": format!("Model loaded from {}", request.model_path)
                    }));
                }
                Err(e) => {
                    return Json(serde_json::json!({
                        "success": false,
                        "message": format!("Failed to load model: {}", e)
                    }));
                }
            }
        }
    }
    
    Json(serde_json::json!({
        "success": false,
        "message": "No compatible inference handler found"
    }))
}

async fn websocket_inference(
    ws: WebSocketUpgrade,
    State(state): State<HttpServerState>,
) -> Response {
    ws.on_upgrade(|socket| handle_websocket(socket, state))
}

async fn handle_websocket(
    socket: WebSocket,
    state: HttpServerState,
) {
    let (mut write, mut read) = socket.split();
    let request_id = uuid::Uuid::new_v4().to_string();
    
    while let Some(msg) = read.next().await {
        match msg {
            Ok(WsMessage::Text(text)) => {
                match serde_json::from_str::<WsInferenceRequest>(&text) {
                    Ok(req) => {
                        let created = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();

                        let messages_str: Vec<String> = req.params.messages
                            .iter()
                            .map(|m| format!("{:?}: {}", m.role, m.content))
                            .collect();
                        let prompt = messages_str.join("\n");

                        let tier = tier_from_string(&req.params.model);
                        let inference_req = InferenceRequest {
                            tier,
                            prompt,
                            max_tokens: req.params.max_tokens.unwrap_or(100),
                        };

                        let handlers = state.inference_handlers.read().await;
                        let mut tokens = Vec::new();

                        for handler in handlers.iter() {
                            if handler.supported_tiers().contains(&inference_req.tier) {
                                match handler.handle_request(inference_req.clone()).await {
                                    Ok(resp) => {
                                        tokens = resp.tokens;
                                        break;
                                    }
                                    Err(e) => {
                                        tracing::warn!("Handler error: {:?}", e);
                                    }
                                }
                            }
                        }

                        if tokens.is_empty() {
                            tokens = vec![format!("WS response for: {}", req.params.messages.last().map(|m| m.content.as_str()).unwrap_or(""))];
                        }

                        let model = req.params.model.clone();
                        let response_len: usize = tokens.iter().map(|t| t.len()).sum();
                        
                        for (i, token) in tokens.iter().enumerate() {
                            let chunk = WsStreamChunk {
                                id: req.id.clone(),
                                model: model.clone(),
                                choices: vec![WsStreamChoice {
                                    index: 0,
                                    delta: WsDelta {
                                        content: token.clone(),
                                        role: if i == 0 { Some("assistant".to_string()) } else { None },
                                    },
                                    finish_reason: if i == tokens.len() - 1 { Some("stop".to_string()) } else { None },
                                }],
                            };
                            
                            if let Ok(json) = serde_json::to_string(&chunk) {
                                let _ = write.send(WsMessage::Text(json)).await;
                            }
                            
                            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                        }

                        let final_response = WsResponse {
                            id: req.id.clone(),
                            method: "inference.result".to_string(),
                            result: Some(WsResult {
                                model,
                                choices: vec![WsChoice {
                                    index: 0,
                                    message: ChatMessage {
                                        role: MessageRole::Assistant,
                                        content: tokens.join(""),
                                    },
                                    finish_reason: "stop".to_string(),
                                }],
                                usage: Usage {
                                    prompt_tokens: (req.params.messages.iter().map(|m| m.content.len()).sum::<usize>() / 4) as u32,
                                    completion_tokens: (response_len / 4) as u32,
                                    total_tokens: ((req.params.messages.iter().map(|m| m.content.len()).sum::<usize>() + response_len) / 4) as u32,
                                },
                                created,
                            }),
                            error: None,
                        };

                        if let Ok(json) = serde_json::to_string(&final_response) {
                            let _ = write.send(WsMessage::Text(json)).await;
                        }
                    }
                    Err(e) => {
                        let error_response = WsResponse {
                            id: request_id.clone(),
                            method: "error".to_string(),
                            result: None,
                            error: Some(WsError {
                                code: -32700,
                                message: format!("Parse error: {}", e),
                            }),
                        };
                        if let Ok(json) = serde_json::to_string(&error_response) {
                            let _ = write.send(WsMessage::Text(json)).await;
                        }
                    }
                }
            }
            Ok(WsMessage::Ping(data)) => {
                let _ = write.send(WsMessage::Pong(data)).await;
            }
            Ok(WsMessage::Close(_)) => {
                break;
            }
            Err(e) => {
                tracing::error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }
}

pub mod conversion {
    use super::*;

    pub fn inference_response_to_chat_completion(
        response: InferenceResponse,
        model: String,
    ) -> ChatCompletionResponse {
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        ChatCompletionResponse {
            id: format!("chatcmpl-{}", hex::encode(response.request_id.0)),
            object: "chat.completion".to_string(),
            created,
            model,
            choices: vec![ChatCompletionChoice {
                index: 0,
                message: ChatMessage {
                    role: MessageRole::Assistant,
                    content: response.tokens.join(""),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: Usage {
                prompt_tokens: response.usage.prompt_tokens,
                completion_tokens: response.usage.completion_tokens,
                total_tokens: response.usage.total_tokens,
            },
        }
    }
}
