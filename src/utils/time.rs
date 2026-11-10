use crate::error::{AppError, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Safe time utilities that handle system time errors gracefully
pub struct TimeUtils;

impl TimeUtils {
    /// Get current timestamp in seconds since UNIX_EPOCH
    /// Returns a reasonable fallback if system time is invalid (e.g., clock moved backwards)
    pub fn current_timestamp_secs() -> u64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs(),
            Err(e) => {
                warn!("System time error (clock may have moved backwards): {}. Using fallback timestamp.", e);
                // Use a reasonable fallback timestamp (2024-01-01) instead of 0
                1704067200 // 2024-01-01 00:00:00 UTC
            }
        }
    }

    /// Get current timestamp in milliseconds since UNIX_EPOCH
    /// Returns a reasonable fallback if system time is invalid (e.g., clock moved backwards)
    pub fn current_timestamp_millis() -> u64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as u64,
            Err(e) => {
                warn!("System time error (clock may have moved backwards): {}. Using fallback timestamp.", e);
                // Use a reasonable fallback timestamp (2024-01-01) in milliseconds
                1704067200000 // 2024-01-01 00:00:00 UTC in millis
            }
        }
    }

    /// Get current timestamp in seconds since UNIX_EPOCH with error handling
    /// Returns an error if system time is invalid
    pub fn current_timestamp_secs_checked() -> Result<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|e| AppError::Internal(format!("System time error: {}", e)))
    }

    /// Get current timestamp in milliseconds since UNIX_EPOCH with error handling
    /// Returns an error if system time is invalid
    pub fn current_timestamp_millis_checked() -> Result<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .map_err(|e| AppError::Internal(format!("System time error: {}", e)))
    }

    /// Get current timestamp in nanoseconds since UNIX_EPOCH
    /// Returns a reasonable fallback if system time is invalid (e.g., clock moved backwards)
    pub fn current_timestamp_nanos() -> u128 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos(),
            Err(e) => {
                warn!("System time error (clock may have moved backwards): {}. Using fallback timestamp.", e);
                // Use a reasonable fallback timestamp (2024-01-01) in nanoseconds
                1704067200000000000 // 2024-01-01 00:00:00 UTC in nanos
            }
        }
    }
}
