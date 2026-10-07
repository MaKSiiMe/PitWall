//! Typed errors of the GbxRemote client.

use std::time::Duration;

/// Errors raised while converting [`crate::Value`]s.
#[derive(Debug, thiserror::Error)]
pub enum ValueError {
    #[error("number cannot be represented as an XML-RPC value")]
    NumberOutOfRange,
    #[error("failed to deserialize XML-RPC value: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("failed to serialize into an XML-RPC value: {0}")]
    Serialize(#[source] serde_json::Error),
}

/// Errors raised while parsing or writing XML-RPC documents.
#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("malformed XML: {0}")]
    Syntax(#[from] quick_xml::Error),
    #[error("invalid XML-RPC document: {0}")]
    Invalid(String),
    #[error("invalid {kind} literal {literal:?}")]
    BadLiteral { kind: &'static str, literal: String },
}

/// Errors raised by the framing layer.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame of {size} bytes exceeds the {max} bytes limit")]
    TooLarge { size: usize, max: usize },
}

/// A fault returned by the server for a method call.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("fault {code}: {message}")]
pub struct Fault {
    pub code: i32,
    pub message: String,
}

/// Errors returned by [`crate::GbxClient`].
#[derive(Debug, thiserror::Error)]
pub enum GbxError {
    #[error("could not connect to {addr}: {source}")]
    Connect {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Xml(#[from] XmlError),
    #[error(transparent)]
    Value(#[from] ValueError),
    #[error("server returned {0}")]
    Fault(#[from] Fault),
    #[error("call to {method} timed out after {timeout:?}")]
    Timeout { method: String, timeout: Duration },
    #[error("connection to the server is closed")]
    ConnectionClosed,
    #[error("unexpected response to {method}: expected {expected}, got {got}")]
    UnexpectedResponse {
        method: String,
        expected: &'static str,
        got: String,
    },
}
