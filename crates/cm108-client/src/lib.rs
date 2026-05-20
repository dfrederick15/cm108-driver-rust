mod client;
mod ffi;
mod framing;

pub use client::Cm108Client;
pub use ffi::Cm108Event;

// Re-export types callers need when using the Rust API directly.
pub use cm108_types::{AudioFrame, RadioEvent, ServerMsg, StreamFlags};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("server disconnected")]
    Disconnected,
}

pub type Result<T> = std::result::Result<T, ClientError>;
