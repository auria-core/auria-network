// File: lib.rs - This file is part of AURIA
// Copyright (c) 2026 AURIA Developers and Contributors
// Description:
//     HTTP and gRPC networking for AURIA Runtime Core.
//     Provides network server implementation for handling external requests
//     via HTTP (OpenAI-compatible API) and gRPC protocols.
//
use auria_core::AuriaResult;

pub struct NetworkServer {
    http_port: u16,
    grpc_port: u16,
}

impl NetworkServer {
    pub fn new(http_port: u16, grpc_port: u16) -> Self {
        Self {
            http_port,
            grpc_port,
        }
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
}
