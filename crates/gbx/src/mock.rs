//! In-process fake GbxRemote 2 server for tests and offline demos.
//!
//! It speaks the real wire protocol, answers calls through a handler and can push callbacks.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use crate::error::Fault;
use crate::frame::{Frame, GbxCodec, PROTOCOL_HEADER};
use crate::value::Value;
use crate::xml::{self, MethodCall, MethodResponse};

type Handler = dyn Fn(&MethodCall) -> MethodResponse + Send + Sync;

/// A fake dedicated server listening on `127.0.0.1`.
pub struct MockServer {
    addr: SocketAddr,
    calls: Arc<Mutex<Vec<MethodCall>>>,
    callbacks: broadcast::Sender<Bytes>,
    stop: CancellationToken,
}

impl Drop for MockServer {
    /// Dropping the server closes the listener and every connection.
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl std::fmt::Debug for MockServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// Default handler: every call succeeds with `true`, except a few getters with plausible values.
pub fn default_handler(call: &MethodCall) -> MethodResponse {
    let s = |pairs: &[(&str, Value)]| {
        Value::Struct(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    };
    Ok(match call.method.as_str() {
        "GetVersion" => s(&[
            ("Name", "Trackmania".into()),
            ("TitleId", "Trackmania".into()),
            ("Version", "3.3.0".into()),
            ("Build", "mock".into()),
            ("ApiVersion", "2023-04-24".into()),
        ]),
        "GetScriptName" => s(&[
            (
                "CurrentValue",
                "Trackmania/TM_TimeAttack_Online.Script.txt".into(),
            ),
            (
                "NextValue",
                "Trackmania/TM_TimeAttack_Online.Script.txt".into(),
            ),
        ]),
        "GetPlayerList" | "GetMapList" | "GetGuestList" => Value::Array(vec![]),
        "GetModeScriptSettings" => Value::Struct(Default::default()),
        "GetCurrentMapInfo" => s(&[("UId", "mockuid".into()), ("Name", "Mock map".into())]),
        "GetMapsDirectory" => "/server/UserData/Maps/".into(),
        "LoadMatchSettings" | "SaveMatchSettings" => 1.into(),
        "Authenticate" => {
            let password = call.params.get(1).and_then(Value::as_str);
            if password == Some("wrong") {
                return Err(Fault {
                    code: -1000,
                    message: "Login unknown or bad password.".to_owned(),
                });
            }
            true.into()
        }
        _ => true.into(),
    })
}

impl MockServer {
    /// Starts a server with [`default_handler`].
    pub async fn start() -> std::io::Result<MockServer> {
        Self::with_handler(default_handler).await
    }

    /// Starts a server answering calls with `handler`.
    pub async fn with_handler<F>(handler: F) -> std::io::Result<MockServer>
    where
        F: Fn(&MethodCall) -> MethodResponse + Send + Sync + 'static,
    {
        Self::with_handler_on("127.0.0.1:0", handler).await
    }

    /// Same as [`MockServer::with_handler`] on a given address (to simulate a server restart).
    pub async fn with_handler_on<F>(bind: &str, handler: F) -> std::io::Result<MockServer>
    where
        F: Fn(&MethodCall) -> MethodResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind(bind).await?;
        let addr = listener.local_addr()?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (callbacks, _) = broadcast::channel::<Bytes>(1024);
        let handler: Arc<Handler> = Arc::new(handler);

        let accept_calls = Arc::clone(&calls);
        let accept_callbacks = callbacks.clone();
        let stop = CancellationToken::new();
        let accept_stop = stop.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    () = accept_stop.cancelled() => break,
                };
                let Ok((stream, _)) = accepted else { break };
                let connection_stop = accept_stop.clone();
                let calls = Arc::clone(&accept_calls);
                let handler = Arc::clone(&handler);
                let mut callback_rx = accept_callbacks.subscribe();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let header = PROTOCOL_HEADER.as_bytes();
                    let mut hello = (header.len() as u32).to_le_bytes().to_vec();
                    hello.extend_from_slice(header);
                    if write.write_all(&hello).await.is_err() {
                        return;
                    }
                    let mut reader = FramedRead::new(read, GbxCodec::default());
                    let mut writer = FramedWrite::new(write, GbxCodec::default());
                    let mut callback_handle: u32 = 0;
                    loop {
                        tokio::select! {
                            () = connection_stop.cancelled() => break,
                            frame = reader.next() => {
                                let Some(Ok(frame)) = frame else { break };
                                let text = String::from_utf8_lossy(&frame.payload);
                                let response = match xml::decode_call(&text) {
                                    Ok(call) => {
                                        let response = handler(&call);
                                        calls.lock().unwrap_or_else(PoisonError::into_inner).push(call);
                                        response
                                    }
                                    Err(err) => Err(Fault { code: -503, message: err.to_string() }),
                                };
                                let payload = Bytes::from(xml::encode_response(&response));
                                if writer.send(Frame { handle: frame.handle, payload }).await.is_err() {
                                    break;
                                }
                            }
                            callback = callback_rx.recv() => {
                                let Ok(payload) = callback else { break };
                                callback_handle = callback_handle.wrapping_add(1) & 0x7FFF_FFFF;
                                if writer.send(Frame { handle: callback_handle, payload }).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });

        Ok(MockServer {
            addr,
            calls,
            callbacks,
            stop,
        })
    }

    /// Closes the listener and every connection, like a crashed server.
    pub fn shutdown(&self) {
        self.stop.cancel();
    }

    /// `host:port` to connect to.
    pub fn addr(&self) -> String {
        self.addr.to_string()
    }

    /// Pushes a legacy callback to every connected client.
    pub fn send_callback(&self, method: &str, params: Vec<Value>) {
        let payload = Bytes::from(xml::encode_call(method, &params));
        let _ = self.callbacks.send(payload);
    }

    /// Pushes a mode script callback (`ManiaPlanet.ModeScriptCallbackArray`).
    pub fn send_script_callback(&self, name: &str, json: &serde_json::Value) {
        self.send_callback(
            "ManiaPlanet.ModeScriptCallbackArray",
            vec![name.into(), Value::Array(vec![json.to_string().into()])],
        );
    }

    /// Calls received so far.
    pub fn calls(&self) -> Vec<MethodCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Names of the methods called so far, in order.
    pub fn method_names(&self) -> Vec<String> {
        self.calls().into_iter().map(|c| c.method).collect()
    }
}
