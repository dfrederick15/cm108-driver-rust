mod client;
mod ffi;

pub use client::Cm108Client;

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
