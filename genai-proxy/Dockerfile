# Build stage
FROM golang:1.24-alpine AS builder

WORKDIR /build

# Copy dependency files
COPY go.mod go.sum* ./
RUN go mod download

# Copy source
COPY cmd/ ./cmd/
COPY internal/ ./internal/

# Build binary
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o genai-proxy ./cmd/proxy

# Runtime stage
FROM alpine:latest

RUN apk --no-cache add ca-certificates

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/genai-proxy .

# Expose port
EXPOSE 8081

# Run
CMD ["./genai-proxy"]

