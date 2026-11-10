package main

import (
	"context"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/ivloli/lsp-gateway/genai-proxy/internal"
)

func main() {
	// 端口配置（唯一的环境变量，默认 8081）
	port := os.Getenv("PORT")
	if port == "" {
		port = "8081"
	}

	// 创建 HTTP Server
	mux := http.NewServeMux()
	mux.HandleFunc("/generate", internal.HandleGenerate)

	// 健康检查
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("OK"))
	})

	// Wrap with trace middleware to inject request_id into context
	handler := internal.TraceMiddleware(mux)

	server := &http.Server{
		Addr:         ":" + port,
		Handler:      handler,
		ReadTimeout:  60 * time.Second,  // 增加读取超时以支持大请求
		WriteTimeout: 300 * time.Second, // LLM 流式请求可能需要更长时间
		IdleTimeout:  60 * time.Second,
	}

	// 优雅关闭
	go func() {
		sigChan := make(chan os.Signal, 1)
		signal.Notify(sigChan, os.Interrupt, syscall.SIGTERM)
		<-sigChan

		slog.Info("Shutting down server...")
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		if err := server.Shutdown(ctx); err != nil {
			slog.Error("Server shutdown error", "error", err)
		}
	}()

	// 启动服务
	slog.Info("GenAI Proxy starting", "port", port)
	slog.Info("Endpoints:")
	slog.Info("  POST /generate - Generate content")
	slog.Info("  GET  /health   - Health check")

	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		slog.Error("[FATAL] Server error", "error", err)
		os.Exit(1)
	}

	slog.Info("Server stopped")
}
