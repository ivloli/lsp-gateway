//! Chat and text completion handlers
//!
//! This module provides HTTP endpoints for OpenAI-compatible chat and text completions.
//! It supports multiple backend providers, automatic failover, streaming responses,
//! and template-based model formatting.
//!
//! # Module Structure
//!
//! - `endpoints`: HTTP endpoint handlers for `/v1/chat/completions` and `/v1/completions`
//! - `processor`: Unified request processing with provider selection and metrics
//! - `types`: Request/response type definitions
//! - `response`: Response enum types for streaming and non-streaming results
//! - `template_handler`: Handles models that require template formatting
//! - `stream_converter`: Converts between completion and chat stream formats
//! - `utils`: Shared utility functions (request ID extraction, metrics)

mod endpoints;
pub mod processor;
pub mod response;
mod stream_converter;
mod template_handler;
pub mod types;
mod utils;

// Re-export public API to maintain backward compatibility
pub use endpoints::{completions, text_completions};
pub use response::{ChatCompletionResult, CompletionResult};
pub use types::*;
