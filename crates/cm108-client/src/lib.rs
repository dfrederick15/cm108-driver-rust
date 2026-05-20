mod client;
mod ffi;
mod framing;

pub use client::Cm108Client;
pub use ffi::Cm108Event;

pub use cm108_types::{AudioFrame, RadioEvent, ServerMsg, StreamFlags};

use std::fmt;

#[derive(Debug)]
pub enum ClientError {
    Io(std::io::Error),
    Protocol(String),
    Disconnected,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e)        => write!(f, "IO error: {e}"),
            Self::Protocol(s)  => write!(f, "protocol error: {s}"),
            Self::Disconnected => write!(f, "server disconnected"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

pub type Result<T> = std::result::Result<T, ClientError>;
