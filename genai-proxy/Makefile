.PHONY: help build run test clean docker

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'

build: ## Build the binary
	go build -o genai-proxy ./cmd/proxy

run: ## Run the proxy
	go run ./cmd/proxy

test: ## Run tests
	@if [ -z "$$GEMINI_API_KEY" ]; then \
		echo "Error: GEMINI_API_KEY not set"; \
		exit 1; \
	fi
	./scripts/test.sh

clean: ## Clean build artifacts
	rm -f genai-proxy

docker: ## Build Docker image
	docker build -t genai-proxy .

docker-run: ## Run Docker container
	docker run -p 8080:8080 genai-proxy

lines: ## Count lines of code
	@wc -l cmd/proxy/*.go internal/*.go | tail -1

fmt: ## Format code
	go fmt ./...

lint: ## Run linter
	golangci-lint run || go vet ./...

