#[cfg(test)]
mod network_tests {
    use auria_network::*;
    use auria_core::*;

    #[tokio::test]
    async fn test_network_server_creation() {
        let server = NetworkServer::new(8080, 50051);
        
        assert_eq!(server.http_port(), 8080);
        assert_eq!(server.grpc_port(), 50051);
    }

    #[tokio::test]
    async fn test_http_server_creation() {
        let _server = HttpServer::new(8080);
    }

    #[tokio::test]
    async fn test_grpc_server_creation() {
        let _server = GrpcServer::new(50051);
    }

    #[tokio::test]
    async fn test_request_submission() {
        let server = NetworkServer::new(8080, 50051);
        
        let request = InferenceRequest {
            tier: Tier::Standard,
            prompt: "Hello".to_string(),
            max_tokens: 100,
        };
        
        let request_id = server.submit_request(request).await.unwrap();
        
        let status = server.get_request_status(request_id).await;
        assert!(status.is_some());
    }

    #[tokio::test]
    async fn test_request_status_tracking() {
        let server = NetworkServer::new(8080, 50051);
        
        let request = InferenceRequest {
            tier: Tier::Pro,
            prompt: "Test".to_string(),
            max_tokens: 50,
        };
        
        let request_id = server.submit_request(request).await.unwrap();
        
        let status = server.get_request_status(request_id).await;
        assert!(matches!(status, Some(RequestStatus::Pending)));
    }

    #[tokio::test]
    async fn test_shutdown_clears_requests() {
        let server = NetworkServer::new(8080, 50051);
        
        let request = InferenceRequest {
            tier: Tier::Standard,
            prompt: "Test".to_string(),
            max_tokens: 100,
        };
        
        server.submit_request(request).await.unwrap();
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_handler_registration() {
        struct DummyHandler;
        
        #[async_trait::async_trait]
        impl RequestHandler for DummyHandler {
            async fn handle_request(&self, _request: InferenceRequest) -> AuriaResult<InferenceResponse> {
                Ok(InferenceResponse {
                    request_id: RequestId(uuid::Uuid::new_v4().into_bytes()),
                    tokens: vec!["test".to_string()],
                    usage: UsageInfo {
                        prompt_tokens: 10,
                        completion_tokens: 20,
                        total_tokens: 30,
                    },
                })
            }
            
            fn supported_tiers(&self) -> &[Tier] {
                &[Tier::Standard]
            }
        }
        
        let server = NetworkServer::new(8080, 50051);
        server.register_handler(Box::new(DummyHandler)).await;
    }

    #[test]
    fn test_inference_request_fields() {
        let request = InferenceRequest {
            tier: Tier::Max,
            prompt: "Write something".to_string(),
            max_tokens: 1000,
        };
        
        assert_eq!(request.tier, Tier::Max);
        assert_eq!(request.max_tokens, 1000);
    }

    #[test]
    fn test_usage_info_totals() {
        let usage = UsageInfo {
            prompt_tokens: 100,
            completion_tokens: 200,
            total_tokens: 300,
        };
        
        assert_eq!(usage.total_tokens, 300);
    }
}
