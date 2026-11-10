# GenAI Proxy

**A dead simple HTTP proxy for Google GenAI Go SDK.**

## Quick Start

```bash
# Run the proxy
go run ./cmd/proxy

# Or build and run
go build -o genai-proxy ./cmd/proxy
./genai-proxy
```

## Documentation

- [Full Documentation](docs/README.md) - Complete usage guide
- [Architecture](docs/ARCHITECTURE.md) - Design philosophy and principles
- [Quick Start](docs/QUICKSTART.md) - Get started in 5 minutes
- [Backends](docs/BACKENDS.md) - Gemini API vs Vertex AI
- [Changelog](docs/CHANGELOG.md) - Version history

## Features

- ✅ **Simple** - 266 lines of code, zero complexity
- ✅ **Stateless** - Every request is independent
- ✅ **Direct** - Zero data transformation
- ✅ **Fast** - Minimal overhead (~2-7ms)

## Example

```bash
curl -X POST http://localhost:8080/generate \
  -H "Content-Type: application/json" \
  -d '{
    "apiKey": "YOUR_API_KEY",
    "baseURL": "https://aiplatform.googleapis.com",
    "backend": "vertex",
    "model": "gemini-2.0-flash-exp",
    "contents": [{"parts": [{"text": "Hello!"}]}]
  }'
```

## Project Structure

```
genai-proxy/
├── cmd/proxy/       # Entry point
├── internal/        # Core logic
├── docs/            # Documentation
├── examples/        # Client examples
└── scripts/         # Test scripts
```

## License

Same as parent project.
