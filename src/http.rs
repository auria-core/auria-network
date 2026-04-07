// File: http.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     HTTP server implementation for AURIA Runtime Core.
//     Provides OpenAI-compatible REST API for inference requests.

use axum::{
    extract::{State, Path},
    http::StatusCode,
    response::{IntoResponse, Response, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::RwLock;

use crate::{NetworkServer, RequestStatus};
use crate::P2PNode;
use crate::{InferenceRequest, InferenceResponse, RequestHandler, UsageInfo};
use auria_core::{RequestId, Tier};

#[derive(Clone)]
pub struct HttpServerState {
    pub network_server: Arc<NetworkServer>,
    pub p2p_node: Arc<RwLock<Option<P2PNode>>>,
    pub inference_handlers: Arc<RwLock<Vec<Box<dyn RequestHandler>>>>,
}

impl HttpServerState {
    pub fn new(network_server: NetworkServer) -> Self {
        Self {
            network_server: Arc::new(network_server),
            p2p_node: Arc::new(RwLock::new(None)),
            inference_handlers: Arc::new(RwLock::new(Vec::new())),
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
    let mut response_text = String::new();
    
    for handler in handlers.iter() {
        if handler.supported_tiers().contains(&inference_req.tier) {
            match handler.handle_request(inference_req.clone()).await {
                Ok(resp) => {
                    response_text = resp.tokens.join("");
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

    let response_len = response_text.len();
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
            prompt_tokens: (request.messages.iter().map(|m| m.content.len()).sum::<usize>() / 4) as u32,
            completion_tokens: (response_len / 4) as u32,
            total_tokens: ((request.messages.iter().map(|m| m.content.len()).sum::<usize>() + response_len) / 4) as u32,
        },
    };

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
                    response_text = resp.tokens.join("");
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
            .route("/v1/completions", post(completions))
            .route("/v1/models", get(list_models))
            .route("/health", get(health))
            .route("/api/v1/status", get(node_status))
            .route("/api/v1/submit", post(submit_request))
            .route("/api/v1/status/:request_id", get(get_request_status))
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
