//! GbxRemote 2 (XML-RPC) client for the Trackmania 2020 dedicated server.
//!
//! This crate only speaks the protocol: framing, XML-RPC encoding, typed method calls and typed
//! callbacks. It holds no business logic.
//!
//! ```no_run
//! # async fn demo() -> Result<(), pitwall_gbx::GbxError> {
//! use pitwall_gbx::{ClientOptions, GbxClient};
//! let (client, mut callbacks) = GbxClient::connect("127.0.0.1:5000", ClientOptions::default()).await?;
//! client.authenticate("SuperAdmin", "password").await?;
//! client.set_api_version("2023-03-25").await?;
//! client.enable_callbacks(true).await?;
//! while let Some(callback) = callbacks.recv().await {
//!     println!("{callback:?}");
//! }
//! # Ok(()) }
//! ```

pub mod callbacks;
pub mod client;
pub mod error;
pub mod frame;
pub mod methods;
#[cfg(feature = "mock")]
pub mod mock;
pub mod types;
pub mod value;
pub mod xml;

pub use callbacks::{Callback, ScriptCallback};
pub use client::{Callbacks, ClientOptions, GbxClient};
pub use error::{Fault, FrameError, GbxError, ValueError, XmlError};
pub use methods::SpectatorMode;
pub use value::Value;
