package internal

import (
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"time"

	"google.golang.org/genai"
)

// HandleGenerate - 一个 handler 搞定所有请求
// 无状态，零分支复杂度
func HandleGenerate(w http.ResponseWriter, r *http.Request) {
	requestStart := time.Now()
	logger := NewLogger(r.Context())

	// 只接受 POST
	if r.Method != http.MethodPost {
		writeErrorWithLogger(w, logger, http.StatusMethodNotAllowed, "only POST allowed")
		return
	}

	// 解析请求
	var req Request
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		logger.Errorf("[Request Parse Error] elapsed=%v, error=%v", time.Since(requestStart), err)
		writeErrorWithLogger(w, logger, http.StatusBadRequest, fmt.Sprintf("invalid request: %v", err))
		return
	}

	logger.Debugf("[Request Parsed] elapsed=%v, body_read_complete", time.Since(requestStart))

	// API Key 优先级：Header > Request Payload
	// 优先从 Authorization header 获取（Bearer token 或直接 key）
	if apiKey := extractAPIKeyFromHeader(r); apiKey != "" {
		req.APIKey = apiKey
	}

	// 记录请求参数（隐藏敏感信息）
	backend := req.Backend
	if backend == "" {
		backend = "gemini"
	}
	logger.Infof("[Request] backend=%s, model=%s, stream=%v, project=%s, location=%s, apiKey=%s",
		backend, req.Model, req.Stream, req.Project, req.Location, maskAPIKey(req.APIKey))

	// 验证必需参数
	if req.Model == "" {
		writeErrorWithLogger(w, logger, http.StatusBadRequest, "model is required")
		return
	}
	if len(req.Contents) == 0 {
		writeErrorWithLogger(w, logger, http.StatusBadRequest, "contents is required")
		return
	}

	// 验证并修正 stopSequences（Gemini 限制最多 5 个）
	if req.Config != nil && len(req.Config.StopSequences) > 5 {
		logger.Warnf("[Warning] stopSequences has %d elements, truncating to 5 (Gemini limit)",
			len(req.Config.StopSequences))
		req.Config.StopSequences = req.Config.StopSequences[:5]
	}

	// 合并顶层 SafetySettings 到 Config（兼容性处理）
	if len(req.SafetySettings) > 0 {
		if req.Config == nil {
			req.Config = &genai.GenerateContentConfig{}
		}
		// 顶层 SafetySettings 优先（覆盖 Config 中的）
		req.Config.SafetySettings = req.SafetySettings
		logger.Infof("[SafetySettings] merged %d settings from top-level to config", len(req.SafetySettings))
	}

	if req.Config == nil {
		req.Config = &genai.GenerateContentConfig{}
	}

	// 创建 client（每次请求独立创建，简单但正确）
	logger.Infof("[Creating Client] model=%s, backend=%s", req.Model, backend)
	clientStart := time.Now()
	client, err := newClient(r.Context(), &req)
	if err != nil {
		logger.Errorf("[Client Creation Failed] elapsed=%v, error=%v", time.Since(clientStart), err)
		writeErrorWithLogger(w, logger, http.StatusUnauthorized, fmt.Sprintf("client creation failed: %v", err))
		return
	}
	logger.Infof("[Client Created] elapsed=%v", time.Since(clientStart))

	// 根据是否流式，调用不同处理
	if req.Stream {
		handleStream(w, r, client, &req, requestStart, logger)
	} else {
		handleSync(w, r, client, &req, requestStart, logger)
	}
}

// handleSync - 同步请求处理
func handleSync(w http.ResponseWriter, r *http.Request, client *genai.Client, req *Request, requestStart time.Time, logger *Logger) {
	// 调用 SDK
	logger.Infof("[Sync Calling SDK] model=%s, elapsed_since_request=%v", req.Model, time.Since(requestStart))
	sdkStart := time.Now()
	result, err := client.Models.GenerateContent(
		r.Context(),
		req.Model,
		req.Contents,
		req.Config,
	)

	if err != nil {
		logger.Errorf("[Sync Error] model=%s, sdk_elapsed=%v, total_elapsed=%v, error=%v",
			req.Model, time.Since(sdkStart), time.Since(requestStart), err)
		writeErrorWithLogger(w, logger, http.StatusInternalServerError, fmt.Sprintf("generation failed: %v", err))
		return
	}

	// 记录响应信息
	logger.Infof("[Sync Response] model=%s, candidates=%d, sdk_elapsed=%v, total_elapsed=%v",
		req.Model, len(result.Candidates), time.Since(sdkStart), time.Since(requestStart))

	// 直接返回 SDK 响应，零包装
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(Response{Result: result})
}

// handleStream - 流式请求处理
// 关键改进：在设置 HTTP 200 之前先获取第一个 chunk，如果第一个 chunk 就是错误则返回 HTTP 4xx/5xx
func handleStream(w http.ResponseWriter, r *http.Request, client *genai.Client, req *Request, requestStart time.Time, logger *Logger) {
	// 调用 SDK 流式方法
	logger.Infof("[Stream Calling SDK] model=%s, elapsed_since_request=%v", req.Model, time.Since(requestStart))
	sdkStart := time.Now()
	stream := client.Models.GenerateContentStream(
		r.Context(),
		req.Model,
		req.Contents,
		req.Config,
	)

	logger.Infof("[Stream Start] model=%s, sdk_init_elapsed=%v", req.Model, time.Since(sdkStart))

	chunkCount := 0
	var firstChunkTime time.Time
	var lastChunkTime time.Time
	headersWritten := false
	var flusher http.Flusher

	// 🔑 使用 defer 确保即使出错也尝试 flush 缓冲数据
	defer func() {
		if flusher != nil {
			flusher.Flush()
		}
	}()

	// 🔑 关键：只对 stream 进行一次遍历
	// Go 1.23 的 iter.Seq2 迭代器不支持多次遍历
	for resp, err := range stream {
		// 处理错误
		if err != nil {
			if !headersWritten {
				// 第一个 chunk 就是错误，可以返回正确的 HTTP 状态码
				logger.Errorf("[Stream First Chunk Error] model=%s, total_elapsed=%v, error=%v",
					req.Model, time.Since(requestStart), err)
				errStr := err.Error()
				statusCode := http.StatusInternalServerError
				if contains400Keywords(errStr) {
					statusCode = http.StatusBadRequest
				}
				writeErrorWithLogger(w, logger, statusCode, fmt.Sprintf("generation failed: %v", err))
				return
			}
			// 流中间出现错误，只能通过 SSE 发送（HTTP 状态码已经是 200）
			// 简化错误消息，避免发送可能破坏格式的原始数据
			logger.Errorf("[Stream Error] model=%s, chunk=%d, total_elapsed=%v, error=%v",
				req.Model, chunkCount, time.Since(requestStart), err)
			writeSSE(w, "error", `{"error":"stream error"}`)
			flusher.Flush()
			return
		}

		chunkCount++
		now := time.Now()

		// 第一个 chunk 成功，设置 SSE headers
		if !headersWritten {
			w.Header().Set("Content-Type", "text/event-stream")
			w.Header().Set("Cache-Control", "no-cache")
			w.Header().Set("Connection", "keep-alive")

			var ok bool
			flusher, ok = w.(http.Flusher)
			if !ok {
				writeErrorWithLogger(w, logger, http.StatusInternalServerError, "streaming not supported")
				return
			}
			headersWritten = true

			firstChunkTime = now
			logger.Infof("[Stream First Chunk] model=%s, TTFT=%v (time to first token)",
				req.Model, time.Since(sdkStart))

			// 监控 context 状态
			go func() {
				<-r.Context().Done()
				if r.Context().Err() != nil {
					logger.Warnf("[Context Canceled] model=%s, chunks_received=%d, total_elapsed=%v, reason=%v",
						req.Model, chunkCount, time.Since(requestStart), r.Context().Err())
				}
			}()
		}

		timeSinceLastChunk := time.Duration(0)
		if !lastChunkTime.IsZero() {
			timeSinceLastChunk = now.Sub(lastChunkTime)
		}
		lastChunkTime = now

		// 序列化响应
		data, err := json.Marshal(resp)
		if err != nil {
			// 🔑 序列化失败，静默终止避免前端收到格式错误的数据
			// defer flush 会确保已缓冲的数据被发送
			logger.Errorf("[Stream Chunk Marshal Error] model=%s, chunk=%d, error=%v (silent termination)",
				req.Model, chunkCount, err)
			// 直接返回，让流自然结束，defer 会 flush 已有数据
			return
		}

		// 记录每个 chunk（减少日志量，只在特定情况记录）
		if chunkCount <= 3 || chunkCount%10 == 0 || timeSinceLastChunk > 5*time.Second {
			logger.Debugf("[Stream Chunk] model=%s, chunk=%d, size=%d bytes, time_since_last=%v, total_elapsed=%v",
				req.Model, chunkCount, len(data), timeSinceLastChunk, time.Since(requestStart))
		}

		// 🔑 检查客户端是否断开（在写入之前检查）
		if r.Context().Err() != nil {
			logger.Warnf("[Client Disconnected] model=%s, chunk=%d, total_elapsed=%v, reason=%v",
				req.Model, chunkCount, time.Since(requestStart), r.Context().Err())
			return
		}

		// 🔑 发送 SSE 事件，添加错误日志
		_, err = writeSSE(w, "", string(data))
		if err != nil {
			logger.Errorf("[Stream Write Error] model=%s, chunk=%d, error=%v",
				req.Model, chunkCount, err)
			// 写入失败，defer 会确保 flush，这里直接返回
			return
		}

		// 立即 flush 确保客户端收到数据
		flusher.Flush()
	}

	// 如果没有任何响应（空流）
	if chunkCount == 0 {
		logger.Errorf("[Stream Empty] model=%s, total_elapsed=%v", req.Model, time.Since(requestStart))
		writeErrorWithLogger(w, logger, http.StatusInternalServerError, "empty stream response")
		return
	}

	// 流结束
	logger.Infof("[Stream Complete] model=%s, total_chunks=%d, total_elapsed=%v, stream_duration=%v",
		req.Model, chunkCount, time.Since(requestStart), time.Since(firstChunkTime))
	flusher.Flush()
}

// contains400Keywords 检查错误信息是否包含表示 400 错误的关键词
func contains400Keywords(errStr string) bool {
	keywords := []string{
		"400",
		"Bad Request",
		"invalid",
		"Invalid",
		"not enabled",
		"not supported",
		"INVALID_ARGUMENT",
		"Penalty is not enabled",
	}
	for _, kw := range keywords {
		if len(errStr) >= len(kw) {
			for i := 0; i <= len(errStr)-len(kw); i++ {
				if errStr[i:i+len(kw)] == kw {
					return true
				}
			}
		}
	}
	return false
}

// writeSSE - 写 SSE 格式数据
func writeSSE(w io.Writer, event, data string) (int, error) {
	total := 0

	if event != "" {
		n, err := fmt.Fprintf(w, "event: %s\n", event)
		total += n
		if err != nil {
			return total, err
		}
	}
	if data != "" {
		n, err := fmt.Fprintf(w, "data: %s\n", data)
		total += n
		if err != nil {
			return total, err
		}
	}
	n, err := fmt.Fprint(w, "\n")
	total += n
	return total, err
}

// writeError - 统一错误响应 (without context)
func writeError(w http.ResponseWriter, status int, message string) {
	log.Printf("[Error] status=%d, message=%s", status, message)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(ErrorResponse{Error: message})
}

// writeErrorWithLogger - 统一错误响应 (with contextual logger)
func writeErrorWithLogger(w http.ResponseWriter, logger *Logger, status int, message string) {
	logger.Errorf("[Error] status=%d, message=%s", status, message)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(ErrorResponse{Error: message})
}

// extractAPIKeyFromHeader - 从 HTTP header 提取 API Key
// 优先级：Authorization Bearer > Authorization > X-API-Key > Api-Key
func extractAPIKeyFromHeader(r *http.Request) string {
	// 1. Authorization: Bearer <key>
	if auth := r.Header.Get("Authorization"); auth != "" {
		if len(auth) > 7 && auth[:7] == "Bearer " {
			return auth[7:]
		}
		// 2. Authorization: <key> (直接是 key)
		return auth
	}

	// 3. X-API-Key header
	if apiKey := r.Header.Get("X-API-Key"); apiKey != "" {
		return apiKey
	}

	// 4. Api-Key header
	if apiKey := r.Header.Get("Api-Key"); apiKey != "" {
		return apiKey
	}

	return ""
}

// maskAPIKey - 隐藏 API Key 敏感信息
func maskAPIKey(apiKey string) string {
	if len(apiKey) <= 8 {
		return "***"
	}
	return apiKey[:4] + "..." + apiKey[len(apiKey)-4:]
}
