# LSP Gateway

[![CI](https://github.com/ivloli/lsp-gateway/actions/workflows/ci.yml/badge.svg)](https://github.com/ivloli/lsp-gateway/actions/workflows/ci.yml)

A high-performance LLM gateway built in Rust. It provides intelligent routing and
load balancing for OpenAI-compatible chat completion requests across multiple
providers, with adaptive health-based traffic control.

## Features

- **Intelligent routing**: model-based provider selection with optional provider groups and weighted routing
- **Provider groups**: pool providers by `group` with weights and active flags
- **Adaptive traffic control**: health scoring, circuit breaking, and gradual weight adjustment via a Redis-backed supervisor
- **Rate limiting**: token bucket or sliding window algorithms with burst support
- **Caching**: in-memory model config cache for fast lookups
- **OpenAI compatible**: `/v1/models` and `/v1/chat/completions`
- **Streaming support**: streaming chat completions with dual timeouts (first token and total response)
- **Observability**: `/health`, Prometheus `/metrics`, optional OpenTelemetry tracing
- **Multiple provider adapters**: OpenAI, Anthropic, Google (Gemini), OpenRouter, and other OpenAI-compatible APIs
- **Database-driven configuration**: PostgreSQL-backed model and provider definitions
- **Built on Axum + Tokio**

## Quick Start

### Prerequisites

- Rust 1.89+
- PostgreSQL 12+
- Redis (optional; required for rate limiting, metrics aggregation, and the supervisor)

### Setup

1. Clone the repository

   ```bash
   git clone git@github.com:ivloli/lsp-gateway.git
   cd lsp-gateway
   ```

2. Create the database and apply migrations

   ```bash
   createdb lsp_gateway
   psql lsp_gateway -f migrations/001_initial.sql
   ```

3. Configure the environment

   ```bash
   cp .env.example .env
   # edit .env and set DATABASE_URL plus any provider API keys you need
   ```

4. Build and run

   ```bash
   cargo run --release
   ```

The gateway listens on `http://<SERVER_BIND_ADDRESS>:<SERVER_PORT>` (default
`http://localhost:8080`).

### Docker

```bash
docker build -t lsp-gateway .
docker run --rm -p 8080:8080 \
  -e DATABASE_URL=postgresql://host.docker.internal/lsp_gateway \
  -e SERVER_BIND_ADDRESS=0.0.0.0 -e SERVER_PORT=8080 \
  lsp-gateway
```

Or use the bundled compose file, which also starts mock upstream servers:

```bash
docker compose up --build
```

## Endpoints

- `GET /health` — basic health JSON
- `GET /metrics` — Prometheus metrics
- `GET /v1/models` — list active models (OpenAI-compatible shape)
- `POST /v1/chat/completions` — chat completions (streaming and non-streaming)

### Usage Examples

```bash
# Health check
curl http://localhost:8080/health

# Models list
curl http://localhost:8080/v1/models

# Non-streaming chat completion
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "your-model-id",
    "messages": [{"role": "user", "content": "Hello!"}],
    "max_tokens": 100,
    "temperature": 0.7,
    "stream": false
  }'

# Streaming chat completion
# The response is returned as text/plain with concatenated chunks (not SSE).
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "your-model-id",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'
```

## Configuration

Configuration is read from environment variables (optionally via a `.env` file).
See [docs/ENVIRONMENT_VARIABLES.md](docs/ENVIRONMENT_VARIABLES.md) for the full
reference. The most important ones:

| Variable | Default | Description |
| --- | --- | --- |
| `DATABASE_URL` | `postgresql://localhost/lsp_gateway` | PostgreSQL connection URL |
| `REDIS_URL` | `redis://localhost:6379` | Redis connection URL (optional) |
| `SERVER_BIND_ADDRESS` | `0.0.0.0` | HTTP bind address |
| `SERVER_PORT` | `8080` | HTTP port |
| `SERVER_RESPONSE_TIMEOUT_MS` | `55000` | Total streaming response timeout |
| `SERVER_STREAM_NO_TOKEN_TIMEOUT_MS` | `10000` | First-token / inter-token timeout |
| `ROUTING_STRATEGY` | `smooth_weighted_round_robin` | Provider selection strategy |
| `TRACING_ENABLED` | `false` | Enable OpenTelemetry OTLP export |
| `OTLP_TRACES_URL` | `http://localhost:4318` | OTLP collector endpoint |

Provider API keys are read from provider-specific variables such as
`OPENAI_CHAT_API_KEY`, `GOOGLE_AI_API_KEY`, `DEEPSEEK_API_KEY`, and
`OPENROUTER_API_KEY`. Set only the ones you use.

## Database

Two tables drive routing (see `migrations/001_initial.sql`):

- `ModelConfig` — available models and their attributes (`provider`, `baseUrl`,
  `modelName`, `live`, `display`, `modelProviderGroup`, `parameters`, ...)
- `ModelProvider` — provider instances grouped by `group` with `weight`,
  `active`, and base URL

### Provider Groups

1. Set `modelProviderGroup` on the `ModelConfig` row.
2. Create one or more `ModelProvider` rows with the same `group`.
3. Set `active=true` to include a provider in routing and `weight` to control
   its share of traffic.

Migrations are not applied automatically; run the SQL files manually before
starting the binary.

## Architecture

```
Client
  ↓
Axum HTTP handlers (/health, /metrics, /v1/models, /v1/chat/completions)
  ↓
ProviderService (PostgreSQL-backed, in-memory cache)
  ↓
Provider selection (direct or provider group via weights/filters)
  ↓
HTTP client (per-provider auth, streaming with dual timeouts)
  ↓
Upstream LLM provider

Optional: Redis-backed metrics + supervisor for adaptive health/weights → Prometheus /metrics
```

### Circuit Breaker Flow

```
    health_score >= 0.3 (healthy)
CLOSED ←─────────────────────────── HALFOPEN
  │                                    ↑    │
  │ health_score < 0.1 (unhealthy)     │    │ health_score < 0.1
  │                                    │    │ (after min_duration)
  ↓                                    │    ↓
OPEN ──────────────────────────────────┘  OPEN
    timeout >= 30s (recovery attempt)
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for a detailed walkthrough of the routing
pipeline and the traffic-control algorithm.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets
cargo test --all-targets
```

The `genai-proxy/` directory contains an optional Go service that adapts Google
Gemini requests. It is built separately:

```bash
cd genai-proxy && go build ./...
```

## License

MIT — see [LICENSE](LICENSE).
