package internal

import "google.golang.org/genai"

// Request - 所有参数在一个结构体里，零特殊情况
type Request struct {
	// 认证信息 - 每次请求传入，不依赖环境变量
	// APIKey 优先级：HTTP Header > Request Payload
	// 支持 Header: Authorization (Bearer token), X-API-Key, Api-Key
	APIKey  string `json:"apiKey"`            // Gemini API Key 或留空（Vertex AI 用 ADC）
	BaseURL string `json:"baseURL,omitempty"` // 可选，默认官方 API
	Proxy   string `json:"proxy,omitempty"`   // 可选，HTTP/HTTPS/SOCKS5 代理

	// Backend 选择
	Backend  string `json:"backend,omitempty"`  // "gemini" (默认) 或 "vertex"
	Project  string `json:"project,omitempty"`  // Vertex AI: GCP Project ID
	Location string `json:"location,omitempty"` // Vertex AI: GCP Location (如 us-central1)

	// SDK 参数 - 直接映射，零转换
	Model    string                       `json:"model"`
	Contents []*genai.Content             `json:"contents"`
	Config   *genai.GenerateContentConfig `json:"config,omitempty"`

	// 顶层 SafetySettings（兼容性支持）
	// 如果传入，会被合并到 Config.SafetySettings 中
	SafetySettings []*genai.SafetySetting `json:"safetySettings,omitempty"`

	// 流式控制
	Stream bool `json:"stream,omitempty"`

	// 超时配置（从调用方传入）
	// StreamNoTokenTimeoutMs: 流式响应中，等待第一个 token / 响应头的最大时间（毫秒）
	// 用于 Transport.ResponseHeaderTimeout
	StreamNoTokenTimeoutMs int64 `json:"streamNoTokenTimeoutMs,omitempty"`

	// ResponseFinishTimeoutMs: 整体响应完成的最大时间（毫秒）
	// 用于 http.Client.Timeout，如果超过这个时间整个请求会被取消
	// 默认 55 秒
	ResponseFinishTimeoutMs int64 `json:"responseFinishTimeoutMs,omitempty"`
}

// Response - 直接透传 SDK 响应，零包装
type Response struct {
	Result *genai.GenerateContentResponse `json:"result,omitempty"`
	Error  string                         `json:"error,omitempty"`
}

// ErrorResponse - 统一错误格式
type ErrorResponse struct {
	Error string `json:"error"`
}
