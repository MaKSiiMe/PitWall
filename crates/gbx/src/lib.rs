//! GbxRemote 2 (XML-RPC) client for the Trackmania 2020 dedicated server.
//!
//! This crate only speaks the protocol: framing, XML-RPC encoding, method calls and callbacks.
//! It holds no business logic.

pub mod error;
pub mod frame;
pub mod value;
pub mod xml;

pub use error::{Fault, FrameError, GbxError, ValueError, XmlError};
pub use value::Value;
