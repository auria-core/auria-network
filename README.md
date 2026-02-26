# auria-network

HTTP and gRPC networking for AURIA Runtime Core.

## Protocols

- HTTP (OpenAI-compatible API)
- gRPC (native protocol)
- WebSocket (streaming)

## API Endpoints

```
POST /v1/chat/completions
POST /v1/completions
POST /v1/embeddings
```

## Usage

```rust
use auria_network::NetworkServer;

let server = NetworkServer::new(8080, 50051);
server.start().await?;
```
