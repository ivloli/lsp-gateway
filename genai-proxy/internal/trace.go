package internal

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"strings"

	"github.com/google/uuid"
)

// ContextKey is a custom type for context keys to avoid collisions
type ContextKey string

const (
	// RequestIDKey is the context key for request ID
	RequestIDKey ContextKey = "request_id"
)

// ExtractRequestID extracts request ID from traceparent header or generates a new UUID
//
// Traceparent format (W3C Trace Context): version-trace_id-parent_id-trace_flags
// Example: 00-3e78a79ecf20a3135c33df5af592aecd-464576e952f4d100-01
//
// We extract the trace_id (second part) which is 32 hex characters
// If no valid traceparent found, we generate a new UUID
func ExtractRequestID(r *http.Request) string {
	if traceparent := r.Header.Get("traceparent"); traceparent != "" {
		parts := strings.Split(traceparent, "-")
		if len(parts) >= 2 {
			// Extract the trace_id (second part)
			traceID := parts[1]
			if len(traceID) == 32 {
				return traceID
			}
		}
	}

	// Fallback to UUID if no valid traceparent found
	return uuid.New().String()
}

// TraceMiddleware injects request_id into the request context
func TraceMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestID := ExtractRequestID(r)
		
		// Inject request_id into context
		ctx := context.WithValue(r.Context(), RequestIDKey, requestID)
		
		// Create new request with updated context
		r = r.WithContext(ctx)
		
		// Call next handler
		next.ServeHTTP(w, r)
	})
}

// GetRequestID retrieves request_id from context
func GetRequestID(ctx context.Context) string {
	if requestID, ok := ctx.Value(RequestIDKey).(string); ok {
		return requestID
	}
	return "unknown"
}

var (
	// globalLogger is the global slog.Logger instance
	globalLogger *slog.Logger
	// logLevel is the current log level
	logLevel slog.Level
)

// init initializes the global logger based on LOG_LEVEL environment variable
func init() {
	// Parse LOG_LEVEL from environment (default: INFO)
	levelStr := os.Getenv("LOG_LEVEL")
	if levelStr == "" {
		levelStr = "INFO"
	}

	switch strings.ToUpper(levelStr) {
	case "DEBUG":
		logLevel = slog.LevelDebug
	case "INFO":
		logLevel = slog.LevelInfo
	case "WARN", "WARNING":
		logLevel = slog.LevelWarn
	case "ERROR":
		logLevel = slog.LevelError
	default:
		logLevel = slog.LevelInfo
	}

	// Create slog handler (JSON format for production, Text for development)
	var handler slog.Handler
	logFormat := os.Getenv("LOG_FORMAT")
	if logFormat == "json" {
		handler = slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
			Level: logLevel,
		})
	} else {
		handler = slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{
			Level: logLevel,
		})
	}

	globalLogger = slog.New(handler)
}

// Logger wraps logging with request_id and log levels
type Logger struct {
	ctx    context.Context
	logger *slog.Logger
}

// NewLogger creates a new contextual logger
func NewLogger(ctx context.Context) *Logger {
	requestID := GetRequestID(ctx)
	// Create a logger with request_id as a permanent attribute
	logger := globalLogger.With("request_id", requestID)
	return &Logger{
		ctx:    ctx,
		logger: logger,
	}
}

// Fatal logs at FATAL level and exits (use sparingly)
func (l *Logger) Fatal(msg string, args ...interface{}) {
	l.logger.Error(fmt.Sprintf("[FATAL] "+msg, args...))
	os.Exit(1)
}

// Fatalf logs at FATAL level with format and exits
func (l *Logger) Fatalf(format string, args ...interface{}) {
	l.Fatal(format, args...)
}

// Error logs at ERROR level
func (l *Logger) Error(msg string, args ...interface{}) {
	l.logger.Error(fmt.Sprintf(msg, args...))
}

// Errorf logs at ERROR level with format
func (l *Logger) Errorf(format string, args ...interface{}) {
	l.Error(format, args...)
}

// Warn logs at WARN level
func (l *Logger) Warn(msg string, args ...interface{}) {
	l.logger.Warn(fmt.Sprintf(msg, args...))
}

// Warnf logs at WARN level with format
func (l *Logger) Warnf(format string, args ...interface{}) {
	l.Warn(format, args...)
}

// Info logs at INFO level
func (l *Logger) Info(msg string, args ...interface{}) {
	l.logger.Info(fmt.Sprintf(msg, args...))
}

// Infof logs at INFO level with format
func (l *Logger) Infof(format string, args ...interface{}) {
	l.Info(format, args...)
}

// Debug logs at DEBUG level
func (l *Logger) Debug(msg string, args ...interface{}) {
	l.logger.Debug(fmt.Sprintf(msg, args...))
}

// Debugf logs at DEBUG level with format
func (l *Logger) Debugf(format string, args ...interface{}) {
	l.Debug(format, args...)
}

// Printf logs at INFO level (backward compatibility)
func (l *Logger) Printf(format string, args ...interface{}) {
	l.logger.Info(fmt.Sprintf(format, args...))
}

// Println logs at INFO level (backward compatibility)
func (l *Logger) Println(args ...interface{}) {
	l.logger.Info(fmt.Sprint(args...))
}

// InjectTraceHeaders injects trace context headers into HTTP request
// This propagates the trace context to downstream services
func InjectTraceHeaders(req *http.Request, ctx context.Context) {
	requestID := GetRequestID(ctx)
	
	// If we have a valid request_id, construct a traceparent header
	// Format: version-trace_id-parent_id-trace_flags
	// We use the request_id as trace_id and generate a new span_id
	if requestID != "" && requestID != "unknown" {
		// Ensure request_id is 32 hex characters for trace_id
		traceID := requestID
		if len(traceID) != 32 {
			// If it's a UUID (with dashes), remove dashes
			traceID = strings.ReplaceAll(traceID, "-", "")
		}
		
		// Pad or truncate to 32 characters if needed
		if len(traceID) < 32 {
			traceID = fmt.Sprintf("%-32s", traceID)
			traceID = strings.ReplaceAll(traceID, " ", "0")
		} else if len(traceID) > 32 {
			traceID = traceID[:32]
		}
		
		// Generate a new span_id (16 hex characters)
		spanID := uuid.New().String()
		spanID = strings.ReplaceAll(spanID, "-", "")[:16]
		
		// Construct traceparent: version-trace_id-parent_id-trace_flags
		// trace_flags: 01 = sampled, 00 = not sampled
		traceparent := fmt.Sprintf("00-%s-%s-01", traceID, spanID)
		req.Header.Set("traceparent", traceparent)
	}
}

