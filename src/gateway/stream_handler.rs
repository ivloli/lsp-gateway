use crate::error::{AppError, Result};
use crate::utils::TimeUtils;
use bytes::BytesMut;
use futures_util::{Stream, StreamExt};
use memchr::memmem;
use reqwest::Response;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::time::{interval, timeout, MissedTickBehavior};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct StreamTimeoutConfig {
    /// Total response timeout (default: 55 seconds)
    pub response_finish_timeout_ms: u64,
    /// Timeout between stream tokens (default: 10 seconds)
    pub stream_no_token_timeout_ms: u64,
}

impl StreamTimeoutConfig {
    /// Calculate token check interval as stream_no_token_timeout_ms / 3
    pub fn token_check_interval_ms(&self) -> u64 {
        std::cmp::min(self.stream_no_token_timeout_ms / 4, 500).max(100)
    }
}

impl Default for StreamTimeoutConfig {
    fn default() -> Self {
        Self {
            response_finish_timeout_ms: 55_000, // 55 seconds
            stream_no_token_timeout_ms: 5_000,  // 5 seconds
        }
    }
}

/// A stream wrapper that implements timeout functionality and SSE boundary handling
///
/// 🚀 Optimizations:
/// - Batch output: Returns all complete SSE messages at once instead of one per poll
/// - Fast passthrough: When buffer ends with SSE boundary, outputs immediately
/// - Zero-copy: Uses BytesMut for zero-copy buffer slicing
/// - memchr: Uses SIMD-accelerated byte search for SSE boundary detection
/// - Multiple boundaries: Handles both \n\n and \r\n\r\n SSE formats
pub struct TimeoutStream<S> {
    inner: S,
    config: StreamTimeoutConfig,
    last_token_time: Arc<AtomicU64>,
    start_time: Instant,
    timeout_checker: tokio::time::Interval,
    buffer: BytesMut, // 🚀 Zero-copy buffer using BytesMut
    timeout_occurred: bool,
    first_chunk_received: bool,
}

impl<S> TimeoutStream<S>
where
    S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    pub fn new(stream: S, config: StreamTimeoutConfig) -> Result<Self> {
        let last_token_time = Arc::new(AtomicU64::new(
            TimeUtils::current_timestamp_millis_checked()?,
        ));
        let start_time = Instant::now();

        let token_check_interval = config.token_check_interval_ms();
        let mut timeout_checker = interval(Duration::from_millis(token_check_interval));
        timeout_checker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        Ok(Self {
            inner: stream,
            config,
            last_token_time,
            start_time,
            timeout_checker,
            buffer: BytesMut::with_capacity(16384), // 🚀 Pre-allocate 16KB zero-copy buffer
            timeout_occurred: false,
            first_chunk_received: false,
        })
    }

    fn check_timeouts(&mut self) -> Result<()> {
        // Check overall response timeout
        if self.start_time.elapsed() > Duration::from_millis(self.config.response_finish_timeout_ms)
        {
            warn!(
                "🚨 SERVER_RESPONSE_TIMEOUT_MS ({}ms) exceeded in streaming",
                self.config.response_finish_timeout_ms
            );
            return Err(AppError::ResponseFinishTimeout {
                timeout_ms: self.config.response_finish_timeout_ms,
            });
        }

        // Check token timeout
        let current_time = TimeUtils::current_timestamp_millis_checked()?;
        let last_time = self.last_token_time.load(Ordering::Relaxed);
        let time_since_last_token_ms = current_time.saturating_sub(last_time);

        if time_since_last_token_ms > self.config.stream_no_token_timeout_ms {
            warn!(
                "🚨 SERVER_STREAM_NO_TOKEN_TIMEOUT_MS ({}ms) exceeded in streaming",
                self.config.stream_no_token_timeout_ms
            );
            return Err(AppError::StreamTokenTimeout {
                timeout_ms: self.config.stream_no_token_timeout_ms,
            });
        }

        Ok(())
    }

    /// Check if buffer ends with a valid SSE boundary (\n\n or \r\n\r\n)
    #[inline]
    fn ends_with_sse_boundary(&self) -> bool {
        self.buffer.ends_with(b"\n\n") || self.buffer.ends_with(b"\r\n\r\n")
    }

    /// 🚀 Fast passthrough: Check if entire buffer ends with complete SSE boundary
    /// If so, we can output the entire buffer without parsing individual messages
    #[inline]
    fn try_fast_passthrough(&mut self) -> Option<bytes::Bytes> {
        // Fast path: if buffer ends with SSE boundary, all messages are complete
        if self.ends_with_sse_boundary() {
            // 🚀 Zero-copy: split_to moves ownership without copying
            let output = self.buffer.split_to(self.buffer.len());
            debug!("🚀 Fast passthrough: {} bytes", output.len());
            return Some(output.freeze());
        }
        None
    }

    /// Find the last SSE boundary position in the buffer
    ///
    /// Returns the end position (exclusive) of the last complete SSE message.
    /// Handles both `\n\n` (LF) and `\r\n\r\n` (CRLF) boundary formats.
    ///
    /// 🚀 Optimization: Uses `memmem::rfind` to scan from the end - finds the last
    /// boundary immediately without traversing all boundaries in the buffer.
    fn find_last_boundary(&self) -> Option<usize> {
        let len = self.buffer.len();

        // Edge case: need at least 2 bytes for the shortest boundary (\n\n)
        if len < 2 {
            return None;
        }

        // 🚀 Use rfind to search from the end - stops at first (i.e., last) match
        // This is O(n) worst case but typically much faster as boundaries are near the end

        // Note: \n\n and \r\n\r\n don't overlap (\r\n\r\n = 0x0D 0x0A 0x0D 0x0A has no consecutive 0x0A 0x0A)
        // So we can safely check both and take the maximum end position

        let lf_end = memmem::rfind(&self.buffer, b"\n\n").map(|pos| pos + 2); // +2 for "\n\n" length

        let crlf_end = if len >= 4 {
            memmem::rfind(&self.buffer, b"\r\n\r\n").map(|pos| pos + 4) // +4 for "\r\n\r\n" length
        } else {
            // Buffer too short for CRLF boundary
            None
        };

        // Return the furthest (maximum) boundary end position
        // This handles the case where buffer contains both formats (rare but possible)
        match (lf_end, crlf_end) {
            (Some(lf), Some(crlf)) => Some(lf.max(crlf)),
            (Some(lf), None) => Some(lf),
            (None, Some(crlf)) => Some(crlf),
            (None, None) => None,
        }
    }

    /// 🚀 Batch extract: Find all complete SSE messages and return them as a single chunk
    /// Uses memchr for SIMD-accelerated boundary detection
    /// Zero-copy using BytesMut::split_to
    fn extract_all_complete_messages(&mut self) -> Option<bytes::Bytes> {
        let last_boundary_end = self.find_last_boundary()?;

        // 🚀 Zero-copy extraction: split_to moves bytes without copying
        let complete_data = self.buffer.split_to(last_boundary_end);

        debug!(
            "🚀 Batch extract: {} bytes, remaining buffer: {} bytes",
            complete_data.len(),
            self.buffer.len()
        );

        Some(complete_data.freeze())
    }
}

impl<S> Stream for TimeoutStream<S>
where
    S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    type Item = std::result::Result<bytes::Bytes, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // 🔑 If timeout already occurred, end stream immediately
        if self.timeout_occurred {
            warn!("Stream already timed out, ending stream");
            return Poll::Ready(None);
        }

        // Check timeouts first
        if let Err(e) = self.check_timeouts() {
            error!("❌ Stream timeout detected: {}", e);
            self.timeout_occurred = true;
            warn!("Terminating stream due to timeout");
            return Poll::Ready(None);
        }

        loop {
            // Poll the inner stream
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if !chunk.is_empty() {
                        // Record first chunk received for debugging
                        if !self.first_chunk_received {
                            let _span = tracing::info_span!(
                                "first_chunk_received",
                                chunk_size = chunk.len()
                            )
                            .entered();
                            self.first_chunk_received = true;
                        }

                        // Update last token time
                        if let Ok(current_time) = TimeUtils::current_timestamp_millis_checked() {
                            self.last_token_time.store(current_time, Ordering::Relaxed);
                        }

                        // 🚀 Zero-copy append using BytesMut
                        self.buffer.extend_from_slice(&chunk);

                        debug!(
                            "Received streaming chunk: {} bytes, buffer size: {}",
                            chunk.len(),
                            self.buffer.len()
                        );

                        // 🚀 Fast passthrough: If buffer ends with SSE boundary, output everything
                        if let Some(output) = self.try_fast_passthrough() {
                            return Poll::Ready(Some(Ok(output)));
                        }

                        // 🚀 Batch extract: Get all complete SSE messages at once
                        if let Some(output) = self.extract_all_complete_messages() {
                            return Poll::Ready(Some(Ok(output)));
                        }

                        // No complete messages yet, continue reading
                        continue;
                    }
                    // Empty chunk, continue
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    let elapsed = self.start_time.elapsed();
                    error!("❌ Stream chunk error after {:?}: {} (is_timeout: {}, is_connect: {}, is_request: {})", 
                        elapsed, e, e.is_timeout(), e.is_connect(), e.is_request());
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => {
                    // Stream ended - handle remaining buffer data
                    if !self.buffer.is_empty() {
                        // 🔑 Only flush if buffer ends with valid SSE boundary
                        // Incomplete data would cause JSON parse errors downstream
                        if self.ends_with_sse_boundary() {
                            let len = self.buffer.len();
                            let remaining_data = self.buffer.split_to(len);
                            debug!(
                                "Stream ended, flushing remaining {} bytes (complete SSE)",
                                remaining_data.len()
                            );
                            return Poll::Ready(Some(Ok(remaining_data.freeze())));
                        } else {
                            // ⚠️ Discard incomplete data - sending it would cause parse errors
                            let discarded_len = self.buffer.len();
                            let preview_len = std::cmp::min(100, discarded_len);
                            let preview = String::from_utf8_lossy(&self.buffer[..preview_len]);
                            warn!(
                                "⚠️ Stream ended with incomplete SSE data ({} bytes), discarding to prevent downstream parse errors. Preview: {}...",
                                discarded_len,
                                preview
                            );
                            self.buffer.clear();
                        }
                    }

                    info!("Stream completed successfully");
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    // 🚀 Before returning Pending, try to extract any complete messages we have
                    if let Some(output) = self.extract_all_complete_messages() {
                        return Poll::Ready(Some(Ok(output)));
                    }

                    // Check timeouts during pending
                    while self.timeout_checker.poll_tick(cx).is_ready() {
                        if let Err(e) = self.check_timeouts() {
                            error!("❌ Stream timeout detected in Pending: {}", e);
                            self.timeout_occurred = true;
                            return Poll::Ready(None);
                        }
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct StreamHandler {
    config: StreamTimeoutConfig,
}

impl StreamHandler {
    pub fn new(config: StreamTimeoutConfig) -> Self {
        Self { config }
    }

    pub fn with_default_config() -> Self {
        Self::new(StreamTimeoutConfig::default())
    }

    /// Process a streaming response with timeout as a real stream
    /// Returns a stream that yields chunks in real-time with timeout protection
    pub fn process_stream_with_timeout_streaming<S>(&self, stream: S) -> Result<TimeoutStream<S>>
    where
        S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
    {
        debug!("Starting real-time stream processing with dual timeout mechanism");
        debug!("Config: {:?}", self.config);
        info!(
            "🔗 Initiating real-time stream connection with SERVER_STREAM_NO_TOKEN_TIMEOUT_MS={}ms",
            self.config.stream_no_token_timeout_ms
        );

        TimeoutStream::new(stream, self.config.clone())
    }

    /// Process a streaming response with dual timeout mechanism
    /// Returns the complete response text or a partial response if timeout occurs
    pub async fn process_stream_with_timeout(&self, response: Response) -> Result<String> {
        debug!("Starting stream processing with dual timeout mechanism");
        debug!("Config: {:?}", self.config);
        info!(
            "🔗 Initiating stream connection with SERVER_STREAM_NO_TOKEN_TIMEOUT_MS={}ms",
            self.config.stream_no_token_timeout_ms
        );

        let mut stream = response.bytes_stream();
        let mut response_buffer = String::new();
        let last_token_time = Arc::new(AtomicU64::new(
            TimeUtils::current_timestamp_millis_checked()?,
        ));
        let start_time = Instant::now();

        // Create timeout checker (check every stream_token_timeout_ms / 3)
        let token_check_interval = self.config.token_check_interval_ms();
        let mut token_timeout_checker = interval(Duration::from_millis(token_check_interval));
        token_timeout_checker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // Clone for the timeout checker
        let last_token_time_clone = Arc::clone(&last_token_time);

        // Race between stream processing and timeouts
        let stream_result = tokio::select! {
            // Main stream processing
            result = self.process_stream_chunks(&mut stream, &mut response_buffer, &last_token_time) => {
                match result {
                    Ok(_) => {
                        if response_buffer.is_empty() {
                            return Err(AppError::Provider("Empty LLM response".to_string()));
                        }
                        debug!("Stream completed successfully. Response length: {}", response_buffer.len());
                        Ok(response_buffer)
                    }
                    Err(e) => {
                        // If we have partial content, return it instead of failing
                        if !response_buffer.is_empty() {
                            warn!("Stream error occurred but returning partial response: {}", e);
                            Ok(response_buffer)
                        } else {
                            Err(e)
                        }
                    }
                }
            }

            // Token timeout checker
            _ = self.check_token_timeout(&mut token_timeout_checker, last_token_time_clone) => {
                warn!("🚨 SERVER_STREAM_NO_TOKEN_TIMEOUT_MS ({}ms) exceeded - actively disconnecting from server", self.config.stream_no_token_timeout_ms);
                // Force drop the stream to close the connection
                drop(stream);
                if !response_buffer.is_empty() {
                    warn!("⚠️ Connection terminated due to stream timeout, had partial response ({} bytes)", response_buffer.len());
                } else {
                    error!("❌ Connection terminated due to stream timeout with no data received");
                }
                // 🔑 关键修复: 无论是否有部分数据，超时都应该返回错误（与TS行为一致）
                Err(AppError::StreamTokenTimeout {
                    timeout_ms: self.config.stream_no_token_timeout_ms
                })
            }

            // Overall response timeout
            _ = tokio::time::sleep(Duration::from_millis(self.config.response_finish_timeout_ms)) => {
                warn!("🚨 SERVER_RESPONSE_TIMEOUT_MS ({}ms) exceeded - actively disconnecting from server", self.config.response_finish_timeout_ms);
                // Force drop the stream to close the connection
                drop(stream);
                if !response_buffer.is_empty() {
                    warn!("⚠️ Connection terminated due to response timeout, had partial response ({} bytes)", response_buffer.len());
                } else {
                    error!("❌ Connection terminated due to response timeout with no data received");
                }
                // 🔑 关键修复: 无论是否有部分数据，超时都应该返回错误（与TS行为一致）
                Err(AppError::ResponseFinishTimeout {
                    timeout_ms: self.config.response_finish_timeout_ms
                })
            }
        };

        let elapsed = start_time.elapsed();
        match &stream_result {
            Err(AppError::StreamTokenTimeout { timeout_ms }) => {
                warn!(
                    "⏰ Stream terminated due to token timeout ({}ms) after {:?}",
                    timeout_ms, elapsed
                );
            }
            Err(AppError::ResponseFinishTimeout { timeout_ms }) => {
                warn!(
                    "⏰ Stream terminated due to response timeout ({}ms) after {:?}",
                    timeout_ms, elapsed
                );
            }
            Err(e) => error!("❌ Stream processing failed after {:?}: {}", elapsed, e),
            _ => {}
        }

        stream_result
    }

    async fn process_stream_chunks<S>(
        &self,
        stream: &mut S,
        response_buffer: &mut String,
        last_token_time: &Arc<AtomicU64>,
    ) -> Result<()>
    where
        S: StreamExt<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
    {
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    if !chunk.is_empty() {
                        // Update last token time using atomic operation
                        let current_time = TimeUtils::current_timestamp_millis_checked()?;
                        last_token_time.store(current_time, Ordering::Relaxed);

                        // Convert bytes to string
                        match std::str::from_utf8(&chunk) {
                            Ok(text) => {
                                response_buffer.push_str(text);
                                debug!("Received chunk: {} bytes", chunk.len());
                            }
                            Err(e) => {
                                warn!("Failed to decode chunk as UTF-8: {}", e);
                                // Still update last_token_time as we received something
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Stream chunk error: {}", e);
                    return Err(AppError::HttpClient(e));
                }
            }
        }
        Ok(())
    }

    async fn check_token_timeout(
        &self,
        token_timeout_checker: &mut tokio::time::Interval,
        last_token_time: Arc<AtomicU64>,
    ) -> Result<()> {
        loop {
            token_timeout_checker.tick().await;

            // Get the current time and last token time using atomic operations
            let current_time = TimeUtils::current_timestamp_millis_checked()?;
            let last_time = last_token_time.load(Ordering::Relaxed);
            let time_since_last_token_ms = current_time.saturating_sub(last_time);

            if time_since_last_token_ms > self.config.stream_no_token_timeout_ms {
                warn!(
                    "🚨 Token timeout detected: {}ms since last token (threshold: {}ms)",
                    time_since_last_token_ms, self.config.stream_no_token_timeout_ms
                );
                info!("🔌 Actively terminating server connection due to SERVER_STREAM_NO_TOKEN_TIMEOUT_MS");
                return Err(AppError::StreamTokenTimeout {
                    timeout_ms: self.config.stream_no_token_timeout_ms,
                });
            }
        }
    }

    /// Process a non-streaming response with simple timeout
    pub async fn process_response_with_timeout(&self, response: Response) -> Result<String> {
        debug!("Processing non-streaming response with timeout");

        let result = timeout(
            Duration::from_millis(self.config.response_finish_timeout_ms),
            response.text(),
        )
        .await;

        match result {
            Ok(Ok(text)) => {
                debug!("Non-streaming response completed. Length: {}", text.len());
                Ok(text)
            }
            Ok(Err(e)) => {
                error!("Failed to read response text: {}", e);
                Err(AppError::HttpClient(e))
            }
            Err(_) => {
                error!(
                    "Response timeout after {}ms",
                    self.config.response_finish_timeout_ms
                );
                Err(AppError::ResponseFinishTimeout {
                    timeout_ms: self.config.response_finish_timeout_ms,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = StreamTimeoutConfig::default();
        assert_eq!(config.response_finish_timeout_ms, 55_000);
        assert_eq!(config.stream_no_token_timeout_ms, 5_000);
        // min(5000/4, 500).max(100) = min(1250, 500).max(100) = 500
        assert_eq!(config.token_check_interval_ms(), 500);
    }

    #[test]
    fn test_custom_config() {
        let config = StreamTimeoutConfig {
            response_finish_timeout_ms: 30_000,
            stream_no_token_timeout_ms: 5_000,
        };
        let handler = StreamHandler::new(config.clone());
        assert_eq!(handler.config.response_finish_timeout_ms, 30_000);
        assert_eq!(handler.config.stream_no_token_timeout_ms, 5_000);
        // min(5000/4, 500).max(100) = 500
        assert_eq!(handler.config.token_check_interval_ms(), 500);
    }

    #[test]
    fn test_token_check_interval_calculation() {
        // Test normal case - capped at 500ms
        let config1 = StreamTimeoutConfig {
            response_finish_timeout_ms: 60_000,
            stream_no_token_timeout_ms: 9_000,
        };
        // min(9000/4, 500).max(100) = min(2250, 500).max(100) = 500
        assert_eq!(config1.token_check_interval_ms(), 500);

        // Test small timeout - uses calculated value
        let config2 = StreamTimeoutConfig {
            response_finish_timeout_ms: 60_000,
            stream_no_token_timeout_ms: 800,
        };
        // min(800/4, 500).max(100) = min(200, 500).max(100) = 200
        assert_eq!(config2.token_check_interval_ms(), 200);

        // Test very small timeout - floor at 100ms
        let config3 = StreamTimeoutConfig {
            response_finish_timeout_ms: 60_000,
            stream_no_token_timeout_ms: 300,
        };
        // min(300/4, 500).max(100) = min(75, 500).max(100) = 100
        assert_eq!(config3.token_check_interval_ms(), 100);
    }

    #[tokio::test]
    async fn test_atomic_timestamp_operations() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        use tokio::time::{sleep, Duration};

        let timestamp = Arc::new(AtomicU64::new(TimeUtils::current_timestamp_millis()));

        let timestamp_clone = Arc::clone(&timestamp);

        // Simulate concurrent access like in the stream handler
        let handle = tokio::spawn(async move {
            for _ in 0..10 {
                let current_time = TimeUtils::current_timestamp_millis();
                timestamp_clone.store(current_time, Ordering::Relaxed);
                sleep(Duration::from_millis(1)).await;
            }
        });

        // Read from main task (like timeout checker)
        for _ in 0..10 {
            let _time = timestamp.load(Ordering::Relaxed);
            sleep(Duration::from_millis(1)).await;
        }

        handle.await.expect("Task should complete successfully");

        // Verify we can still read the timestamp
        let final_time = timestamp.load(Ordering::Relaxed);
        assert!(final_time > 0);
    }

    #[test]
    fn test_sse_boundary_finder_lf() {
        let finder = memmem::Finder::new(b"\n\n");

        // Test finding boundary
        let data = b"data: {\"test\": 1}\n\ndata: {\"test\": 2}\n\n";
        assert_eq!(finder.find(data), Some(17));

        // Test no boundary
        let incomplete = b"data: {\"test\": 1}\n";
        assert_eq!(finder.find(incomplete), None);

        // Test multiple boundaries - finds first
        let multi = b"\n\ndata: test\n\n";
        assert_eq!(finder.find(multi), Some(0));
    }

    #[test]
    fn test_sse_boundary_finder_crlf() {
        let finder = memmem::Finder::new(b"\r\n\r\n");

        // Test finding CRLF boundary
        let data = b"data: {\"test\": 1}\r\n\r\ndata: {\"test\": 2}\r\n\r\n";
        assert_eq!(finder.find(data), Some(17));

        // Test no boundary
        let incomplete = b"data: {\"test\": 1}\r\n";
        assert_eq!(finder.find(incomplete), None);
    }

    #[test]
    fn test_ends_with_sse_boundary() {
        // Test \n\n ending
        let buf1 = BytesMut::from(&b"data: test\n\n"[..]);
        assert!(buf1.ends_with(b"\n\n"));

        // Test \r\n\r\n ending
        let buf2 = BytesMut::from(&b"data: test\r\n\r\n"[..]);
        assert!(buf2.ends_with(b"\r\n\r\n"));

        // Test incomplete - should not match
        let buf3 = BytesMut::from(&b"data: test\n"[..]);
        assert!(!buf3.ends_with(b"\n\n"));
        assert!(!buf3.ends_with(b"\r\n\r\n"));

        // Test partial CRLF
        let buf4 = BytesMut::from(&b"data: test\r\n"[..]);
        assert!(!buf4.ends_with(b"\r\n\r\n"));
    }

    #[test]
    fn test_bytesmut_zero_copy_split() {
        // Verify that split_to works correctly
        let mut buf = BytesMut::with_capacity(100);
        buf.extend_from_slice(b"data: test1\n\ndata: test2\n\n");

        let split = buf.split_to(13); // "data: test1\n\n"

        // The split portion should contain the expected data
        assert_eq!(split.len(), 13);
        assert_eq!(&split[..], b"data: test1\n\n");

        // Remaining buffer should have the rest
        assert_eq!(&buf[..], b"data: test2\n\n");
    }
}
