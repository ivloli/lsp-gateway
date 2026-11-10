package internal

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"

	"google.golang.org/genai"
)

func normalizeBaseURL(baseURL string) string {
	if baseURL == "" {
		return baseURL
	}

	baseURL = strings.TrimSuffix(baseURL, "/")
	versionPattern := regexp.MustCompile(`/v\d+(beta\d*)?$`)
	return versionPattern.ReplaceAllString(baseURL, "")
}

// traceTransport wraps http.RoundTripper to inject trace headers
type traceTransport struct {
	base http.RoundTripper
	ctx  context.Context
}

func (t *traceTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	// Clone the request to avoid modifying the original
	req = req.Clone(req.Context())

	// Inject trace headers
	InjectTraceHeaders(req, t.ctx)

	logger := NewLogger(t.ctx)

	// 记录真实发送的 HTTP 请求
	if req.Body != nil {
		// 读取 body 用于日志记录
		bodyBytes, err := io.ReadAll(req.Body)
		if err == nil {
			// 记录请求（单行 JSON 格式）
			logger.Debugf("[Gemini genai-proxy sent] method=%s, url=%s, body=%s",
				req.Method, req.URL.String(), string(bodyBytes))

			// 重新设置 body，以便 SDK 可以继续使用
			req.Body = io.NopCloser(bytes.NewReader(bodyBytes))
		}
	}

	// Log the actual upstream URL being called
	logger.Infof("[Upstream Request] method=%s, url=%s", req.Method, req.URL.String())

	// Use the base transport to perform the actual request
	startTime := time.Now()
	resp, err := t.base.RoundTrip(req)

	// Log response status
	if err != nil {
		logger.Errorf("[Upstream Error] method=%s, url=%s, elapsed=%v, error=%v",
			req.Method, req.URL.String(), time.Since(startTime), err)
	} else {
		logger.Infof("[Upstream Response] method=%s, url=%s, status=%d, elapsed=%v",
			req.Method, req.URL.String(), resp.StatusCode, time.Since(startTime))
	}

	return resp, err
}

// newClient - 一个函数创建 client，零复杂度
// 每次请求创建新 client，简单但正确
func newClient(ctx context.Context, req *Request) (*genai.Client, error) {
	cfg := &genai.ClientConfig{}

	// 选择 Backend
	backend := req.Backend
	if backend == "" || backend == "gemini" {
		// Gemini API (AI Studio)
		if req.APIKey == "" {
			return nil, fmt.Errorf("apiKey is required for Gemini API")
		}
		cfg.APIKey = req.APIKey
		cfg.Backend = genai.BackendGeminiAPI

	} else if backend == "vertex" {
		// Vertex AI
		// if req.Project == "" {
		// 	return nil, fmt.Errorf("project is required for Vertex AI")
		// }
		// if req.Location == "" {
		// 	return nil, fmt.Errorf("location is required for Vertex AI")
		// }

		cfg.APIKey = req.APIKey
		cfg.Backend = genai.BackendVertexAI
		cfg.Project = req.Project
		cfg.Location = req.Location
		cfg.HTTPOptions.APIVersion = "v1"

	} else {
		return nil, fmt.Errorf("invalid backend: %s (must be 'gemini' or 'vertex')", backend)
	}

	// 如果指定了 baseURL，覆盖默认值
	// 先清理末尾的版本路径（如 /v1, /v1beta），避免与 SDK 自动追加的路径重复
	if req.BaseURL != "" {
		cfg.HTTPOptions.BaseURL = normalizeBaseURL(req.BaseURL)
	}

	// 计算整体超时配置
	// 默认 55 秒
	overallTimeout := 55 * time.Second
	if req.ResponseFinishTimeoutMs > 0 {
		overallTimeout = time.Duration(req.ResponseFinishTimeoutMs) * time.Millisecond
	}
	// 计算超时配置
	// responseHeaderTimeout 按照ResponseFinishTimeoutMs配置走，和node代码保持一致
	// 原本使用req.StreamNoTokenTimeoutMs -> 120s 默认
	responseHeaderTimeout := overallTimeout
	slog.Info(
		"[Client Config] Using ResponseHeaderTimeout (same as ResponseFinishTimeoutMs)",
		"responseHeaderTimeout", responseHeaderTimeout,
	)

	// Create a custom transport that injects trace headers
	var baseTransport http.RoundTripper

	// 如果指定了 proxy，配置 HTTP 代理
	if req.Proxy != "" {
		proxyURL, err := url.Parse(req.Proxy)
		if err != nil {
			return nil, fmt.Errorf("invalid proxy URL: %v", err)
		}
		// 🔑 创建带完整配置的 Transport
		baseTransport = &http.Transport{
			Proxy: http.ProxyURL(proxyURL),
			DialContext: (&net.Dialer{
				Timeout:   30 * time.Second,
				KeepAlive: 30 * time.Second,
			}).DialContext,
			ForceAttemptHTTP2:     true,
			MaxIdleConns:          100,
			IdleConnTimeout:       90 * time.Second,
			TLSHandshakeTimeout:   30 * time.Second,
			ExpectContinueTimeout: 1 * time.Second,
			ResponseHeaderTimeout: responseHeaderTimeout, // 🔑 使用传入的超时配置
		}
	} else {
		// 对于默认 Transport，也需要配置 ResponseHeaderTimeout
		// Clone DefaultTransport 并设置超时
		defaultTransport := http.DefaultTransport.(*http.Transport).Clone()
		defaultTransport.ResponseHeaderTimeout = responseHeaderTimeout
		baseTransport = defaultTransport
	}

	// Wrap the transport with trace injection
	cfg.HTTPClient = &http.Client{
		Transport: &traceTransport{
			base: baseTransport,
			ctx:  ctx,
		},
		Timeout: overallTimeout,
	}

	return genai.NewClient(ctx, cfg)
}
