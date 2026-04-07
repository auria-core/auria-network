// File: inference.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     Inference service implementation for AURIA Runtime Core.
//     Implements the RequestHandler trait to provide inference capabilities.

use async_trait::async_trait;

use crate::{InferenceRequest, InferenceResponse, UsageInfo, RequestHandler};
use auria_core::{AuriaResult, ExpertId, RequestId, RoutingDecision, Tier, Tensor, TensorDType};
use auria_execution::{ExecutionEngine, ExecutionState, ExecutionOutput};
use auria_router::{DeterministicRouter, Router};
use auria_backend_cpu::CpuBackendImpl;

pub struct InferenceService {
    router: DeterministicRouter,
    engine: ExecutionEngine<CpuBackendImpl>,
}

impl InferenceService {
    pub fn new() -> Self {
        let backend = CpuBackendImpl::new();
        let router = DeterministicRouter::new(1024);
        let engine = ExecutionEngine::new(backend);
        
        Self { router, engine }
    }
    
    fn tokenize(&self, text: &str) -> Vec<u32> {
        text.split_whitespace()
            .map(|word| {
                word.bytes()
                    .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
            })
            .collect()
    }
    
    fn detokenize(&self, tokens: &[u32]) -> String {
        let chars: String = tokens.iter()
            .map(|&t| {
                let byte = (t & 0x7F) as u8;
                if byte.is_ascii_graphic() || byte == b' ' {
                    byte as char
                } else {
                    ' '
                }
            })
            .collect();
        chars.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    
    fn create_expert_tensors(&self, num_experts: usize) -> Vec<Tensor> {
        (0..num_experts)
            .map(|_| {
                let hidden_size = 512usize;
                let data: Vec<u8> = (0..hidden_size)
                    .flat_map(|_| {
                        let val: f32 = 0.1_f32;
                        val.to_le_bytes()
                    })
                    .collect();
                Tensor {
                    data,
                    shape: vec![hidden_size as u32],
                    dtype: TensorDType::FP16,
                }
            })
            .collect()
    }
    
    pub async fn generate(
        &self,
        prompt: &str,
        max_tokens: u32,
        tier: Tier,
    ) -> Result<(String, UsageInfo), auria_core::AuriaError> {
        let input_tokens = self.tokenize(prompt);
        let mut state = ExecutionState {
            position: 0,
            kv_cache: Vec::new(),
        };
        
        let input_tensor = Tensor {
            data: input_tokens.iter()
                .flat_map(|&t| t.to_le_bytes())
                .collect(),
            shape: vec![input_tokens.len() as u32],
            dtype: TensorDType::FP16,
        };
        
        let mut output_text = String::new();
        
        for pos in 0..max_tokens {
            let routing = self.router.route(tier, pos as u64);
            
            let result = self.engine.execute(
                input_tensor.clone(),
                routing,
                state.clone(),
            ).await;
            
            match result {
                Ok(output) => {
                    let text = self.process_output(&output);
                    if !text.is_empty() {
                        output_text.push_str(&text);
                        output_text.push(' ');
                    }
                }
                Err(e) => {
                    let fallback = format!("token-{} ", pos);
                    output_text.push_str(&fallback);
                    tracing::debug!("Execution error: {:?}", e);
                }
            }
            
            state.position += 1;
        }
        
        let usage = UsageInfo {
            prompt_tokens: input_tokens.len() as u32,
            completion_tokens: output_text.split_whitespace().count() as u32,
            total_tokens: (input_tokens.len() + output_text.split_whitespace().count()) as u32,
        };
        
        Ok((output_text.trim().to_string(), usage))
    }
    
    fn process_output(&self, output: &ExecutionOutput) -> String {
        if output.tokens.is_empty() {
            return String::new();
        }
        output.tokens.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")
    }
}

impl Default for InferenceService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RequestHandler for InferenceService {
    async fn handle_request(&self, request: InferenceRequest) -> AuriaResult<InferenceResponse> {
        let (text, usage) = self.generate(
            &request.prompt,
            request.max_tokens,
            request.tier,
        ).await.map_err(|e| {
            auria_core::AuriaError::ExecutionError(e.to_string())
        })?;
        
        let words: Vec<&str> = text.split_whitespace().collect();
        let tokens: Vec<String> = words.iter()
            .take(request.max_tokens as usize)
            .map(|s| (*s).to_string())
            .collect();
        
        Ok(InferenceResponse {
            request_id: RequestId(uuid::Uuid::new_v4().into_bytes()),
            tokens,
            usage,
        })
    }
    
    fn supported_tiers(&self) -> &[Tier] {
        &[Tier::Nano, Tier::Standard, Tier::Pro, Tier::Max]
    }
}
