# Environment Variables

All configuration is read from environment variables at startup (a `.env` file
is loaded automatically by `dotenvy`). Every variable is optional and falls back
to the defaults listed below. Invalid numeric or boolean values are silently
replaced by the default.

## Database

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `DATABASE_URL` | string | `postgresql://localhost/lsp_gateway` | PostgreSQL connection URL |
| `DATABASE_MAX_CONNECTIONS` | integer | `10` | Maximum size of the main connection pool |

## Server

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `SERVER_BIND_ADDRESS` | string | `0.0.0.0` | HTTP bind address |
| `SERVER_PORT` | integer | `8080` | HTTP port |
| `SERVER_RESPONSE_TIMEOUT_MS` | integer | `55000` | Maximum total duration of a streaming response (ms) |
| `SERVER_STREAM_NO_TOKEN_TIMEOUT_MS` | integer | `10000` | Maximum time to wait for the first token / next token (ms) |

Both timeout values can be overridden per model via the `ModelConfig` columns
`streamTimeoutMs` and `responseFinishTimeoutMs`.

## Cache

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `CACHE_MAX_CAPACITY` | integer | `10000` | Maximum number of cached model configs |
| `CACHE_TTL_SECONDS` | integer | `30` | Cache entry time-to-live (seconds) |

## Redis

Redis is optional. Without it, rate limiting, metrics aggregation, and the
adaptive supervisor are disabled.

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `REDIS_URL` | string | `redis://localhost:6379` | Redis connection URL |
| `RATELIMIT_REDIS_URL` | string | – | Fallback URL used only when `REDIS_URL` is unset |
| `REDIS_MAX_CONNECTIONS` | integer | `10` | Maximum Redis connection pool size |
| `METRICS_REPORT_INTERVAL_SECONDS` | integer | `3` | Interval for reporting metrics to Redis (seconds) |

## Routing

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `ROUTING_STRATEGY` | string | `smooth_weighted_round_robin` | Weighted provider selection algorithm |

Supported values (unknown values fall back to `smooth_weighted_round_robin`):

| Value | Alias | Behavior |
| --- | --- | --- |
| `smooth_weighted_round_robin` | `swrr` | Deterministic, exactly proportional weight distribution; stateful; ideal for tier failover and standby providers |
| `weighted_power_of_two_choices` | `p2c` | Load-aware: samples two providers and picks the less loaded one; stateless; ideal under variable request durations |
| `weighted_random` | – | Simple weighted random sampling; stateless fallback |

## Rate Limiting

Rate limiting requires Redis and per-provider `max_rps` values.

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `RATE_LIMIT_ALGORITHM` | string | `token_bucket` | `token_bucket` or `sliding_window` (unknown values fall back to `sliding_window`) |
| `BURST_MULTIPLIER` | float | `1.0` | Burst capacity multiplier for `token_bucket` (valid range 1.0–10.0) |

- `token_bucket`: O(1) memory, 3 Redis commands per request, supports bursts.
- `sliding_window`: O(max_rps) memory, 4 Redis commands per request, exact but no burst.

With `max_rps = 1000` and `BURST_MULTIPLIER = 2.0`, the bucket can hold 2000
tokens and absorb short bursts up to 2000 RPS.

## Adaptive Supervisor & Circuit Breaker

The supervisor continuously scores provider health from Redis metrics and
gradually adjusts routing weights. It requires Redis and PostgreSQL.

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `SUPERVISOR_ENABLED` | boolean | `true` | Enable the adaptive supervisor |
| `CIRCUIT_BREAKER_ENABLED` | boolean | `true` | Enable per-provider circuit breaking |
| `SUPERVISOR_CONTROL_INTERVAL_SECS` | integer | `1` | Control-loop interval (seconds) |
| `SUPERVISOR_AGGREGATION_INTERVAL_SECS` | integer | `3` | Metrics aggregation interval (seconds) |
| `SUPERVISOR_GATEWAY_TIMEOUT_SECS` | integer | `60` | Consider a gateway dead after this long (seconds) |
| `SUPERVISOR_CAPACITY_PROTECTION_RATIO` | float | `0.8` | Stop increasing weight above this share of capacity |
| `SUPERVISOR_MAX_ADJUSTMENT_RATIO` | float | `0.5` | Maximum weight change per cycle (0.0–1.0) |
| `SUPERVISOR_CROSS_GATEWAY_BALANCING` | boolean | `true` | Balance traffic across gateway instances |
| `SUPERVISOR_BATCH_SIZE` | integer | `100` | Batch size for Redis operations |
| `SUPERVISOR_ERROR_BACKOFF_SECS` | integer | `5` | Backoff after a control-loop error (seconds) |
| `SUPERVISOR_REDIS_MAX_CONNECTIONS` | integer | `10` | Supervisor Redis pool size |
| `SUPERVISOR_REDIS_TIMEOUT_SECS` | integer | `5` | Supervisor Redis timeout (seconds) |
| `SUPERVISOR_DB_MAX_CONNECTIONS` | integer | `10` | Supervisor database pool size |
| `SUPERVISOR_UNHEALTHY_THRESHOLD` | float | `0.2` | Health score below which a provider is unhealthy |
| `SUPERVISOR_HEALTH_BUFFER` | float | `0.1` | Hysteresis buffer used for half-open recovery |
| `SUPERVISOR_CIRCUIT_RECOVERY_TIMEOUT_SECS` | integer | `30` | Time before an open circuit is retried (seconds) |
| `SUPERVISOR_MIN_HALFOPEN_DURATION_SECS` | integer | `10` | Minimum time spent in half-open state (seconds) |
| `SUPERVISOR_LOCK_ATTEMPT_INTERVAL_SECS` | integer | `15` | Leader-lock acquisition retry interval (seconds) |
| `SUPERVISOR_LOCK_RENEWAL_INTERVAL_SECS` | integer | `10` | Leader-lock renewal interval (seconds) |
| `SUPERVISOR_LOCK_TTL_SECS` | integer | `30` | Leader-lock TTL (seconds) |
| `SUPERVISOR_TRAFFIC_INCREASE_RATE` | float | `0.1` | Weight increase rate while healthy |
| `SUPERVISOR_TRAFFIC_DECREASE_RATE` | float | `0.2` | Weight decrease rate while unhealthy |
| `SUPERVISOR_RECOVERY_WINDOW_SECS` | integer | `600` | Window used to judge recovery (seconds) |
| `SUPERVISOR_MIN_ADJUSTMENT_INTERVAL_SECS` | integer | `1` | Minimum time between weight adjustments (seconds) |
| `SUPERVISOR_ERROR_RATE_THRESHOLD` | float | `0.05` | Error rate above which traffic is reduced |
| `SUPERVISOR_ERROR_RATE_RECOVERY_THRESHOLD` | float | `0.02` | Error rate below which traffic is restored |
| `SUPERVISOR_ERROR_RATE_REL_THRESHOLD` | float | `0.5` | Relative error-rate threshold (50%) |
| `SUPERVISOR_ERROR_RATE_ABS_THRESHOLD` | float | `0.02` | Absolute error-rate threshold (2%) |
| `AGGRESSIVE_WEIGHT_REALLOCATION` | boolean | `false` | Allow larger, faster weight reallocation |

## Retries

Applied to upstream provider requests.

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `RETRY_MAX_RETRIES` | integer | `3` | Maximum retry attempts |
| `RETRY_BASE_BACKOFF_MS` | integer | `1000` | Initial backoff (ms) |
| `RETRY_MAX_BACKOFF_MS` | integer | `10000` | Backoff cap (ms) |
| `RETRY_JITTER_ENABLED` | boolean | `true` | Add random jitter to backoff |
| `RETRY_ON_RATE_LIMIT` | boolean | `true` | Retry when the provider returns 429 |

## Distributed Tracing

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `TRACING_ENABLED` | boolean | `false` | Enable OpenTelemetry OTLP trace export |
| `OTLP_TRACES_URL` | string | `http://localhost:4318` | OTLP/HTTP endpoint (the `/v1/traces` path is appended) |
| `SERVICE_NAME` | string | `lsp-gateway` | Service name reported in traces |
| `SERVICE_VERSION` | string | `0.1.0` | Service version reported in traces |
| `TRACING_SAMPLING_RATIO` | float | `1.0` | Sampling ratio, 0.0–1.0 |
| `TRACING_DEBUG` | boolean | `false` | Verbose span lifecycle logging |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | string | – | Standard OpenTelemetry override for the endpoint |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | string | – | `grpc` or `http/protobuf` |

## GrowthBook (optional feature flags)

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `GROWTHBOOK_ENABLED` | boolean | `false` | Enable GrowthBook feature evaluation |
| `GROWTHBOOK_HOST` | string | `https://cdn.growthbook.io` | GrowthBook API host |
| `GROWTHBOOK_PROXY_HOST` | string | `https://cdn.growthbook.io` | GrowthBook proxy host |
| `GROWTHBOOK_CLIENT_KEY` | string | empty | GrowthBook SDK client key |
| `GROWTHBOOK_REFRESH_INTERVAL_SECS` | integer | `30` | Periodic feature refresh interval (seconds) |
| `GROWTHBOOK_REFRESH_PROBABILITY` | integer | `1000` | 1/N chance to refresh features per request |

## FRP Node Discovery (optional)

When enabled, providers backed by FRP tunnels are discovered through the FRP
dashboard.

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `FRP_ENABLED` | boolean | `false` | Enable FRP node discovery |
| `FRP_URL` | string | `http://localhost:10086` | FRP dashboard API URL |
| `FRP_PASSWD` | string | empty | FRP dashboard password |
| `FRP_CONNS_LIMIT` | integer | `50` | Connection limit per discovered node |
| `FRP_SERVER_SUFFIX` | string | `.frp.example.com` | Domain suffix appended to node names |
| `FRP_SCHEME` | string | `http` | URL scheme used for node endpoints |

## Google / Gemini Adapters

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `GOOGLE_AI_API_KEY` | string | – | API key for direct Google AI (Gemini) requests |
| `GOOGLE_PROXY_URL` | string | `http://localhost:8081` | Base URL of the optional `genai-proxy` service |
| `GEMINI_SYSTEM_INSTRUCTION_MODELS` | JSON | – | Per-adapter lists of model classes that use a system instruction, e.g. `{"google":["gemini-1.5-pro"],"google_proxy":["gemini-1.5-flash"]}` |

## Provider API Keys

Each provider type maps to an environment variable (see `src/client/auth.rs`).
A `ModelConfig` or `ModelProvider` row can override the lookup with its own
`apiKeyEnvName` column.

| Provider type | Variable |
| --- | --- |
| `openai` | `OPENAI_CHAT_API_KEY` |
| `openai_nsfw` | `OPENAI_NSFW_API_KEY` |
| `google` | `GOOGLE_AI_API_KEY` |
| `lepton` | `LEPTON_API_KEY` |
| `sonnet` | `SONNET_API_KEY` |
| `sonnet-provider-2` | `SONNET_API_KEY_BACKUP` |
| `sonnet-provider-3` | `SONNET_API_KEY_BACKUP_2` |
| `other-provider-1` | `OTHER_PROVIDER_1_API_KEY` |
| `other-provider-2` | `OTHER_PROVIDER_2_API_KEY` |
| `deepseek` | `DEEPSEEK_API_KEY` |
| `openrouter` | `OPENROUTER_API_KEY` |
| `novita` | `NOVITA_API_KEY` |
| `featherless` | `FEATHERLESS_API_KEY` |
| `paintbot` | `PAINT_BOT_KEY` |

## Logging

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `RUST_LOG` | string | `info` | `tracing` filter directive (`trace`, `debug`, `info`, `warn`, `error`) |

## Notes

1. Booleans accept `true` / `false`.
2. Numeric values use standard integer or float notation.
3. String values must not be quoted.
4. All variables are optional; built-in defaults are used when unset.
5. `.env` is loaded at startup, but process environment variables take precedence.
