//! Asynchronous GbxRemote 2 client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use crate::callbacks::Callback;
use crate::error::{Fault, GbxError, XmlError};
use crate::frame::{Frame, GbxCodec, HandleAllocator, read_handshake};
use crate::value::Value;
use crate::xml::{self, Message, MethodResponse};

/// Connection options.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Maximum time to wait for a method response.
    pub call_timeout: Duration,
    /// Maximum time to open the TCP connection and read the protocol header.
    pub connect_timeout: Duration,
    /// Size of the callback queue between the socket reader and the consumer.
    pub callback_buffer: usize,
    /// Framing limits.
    pub codec: GbxCodec,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(5),
            callback_buffer: 4096,
            codec: GbxCodec::default(),
        }
    }
}

type Pending = HashMap<u32, oneshot::Sender<Result<MethodResponse, XmlError>>>;
type Writer = FramedWrite<Box<dyn AsyncWrite + Send + Unpin>, GbxCodec>;

struct Inner {
    writer: tokio::sync::Mutex<Writer>,
    pending: Mutex<Pending>,
    handles: Mutex<HandleAllocator>,
    call_timeout: Duration,
    closed: CancellationToken,
}

/// A connected GbxRemote client. Cheap to clone; all clones share the connection.
#[derive(Clone)]
pub struct GbxClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for GbxClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GbxClient")
            .field("closed", &self.inner.closed.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// Stream of callbacks. Ends when the connection closes.
pub type Callbacks = mpsc::Receiver<Callback>;

impl GbxClient {
    /// Connects to a dedicated server over TCP and checks the protocol header.
    pub async fn connect(
        addr: &str,
        options: ClientOptions,
    ) -> Result<(GbxClient, Callbacks), GbxError> {
        let connect = async {
            let stream = TcpStream::connect(addr)
                .await
                .map_err(|source| GbxError::Connect {
                    addr: addr.to_owned(),
                    source,
                })?;
            stream
                .set_nodelay(true)
                .map_err(|source| GbxError::Connect {
                    addr: addr.to_owned(),
                    source,
                })?;
            let (mut read, write) = stream.into_split();
            read_handshake(&mut read).await?;
            Ok::<_, GbxError>((read, write))
        };
        let (read, write) = tokio::time::timeout(options.connect_timeout, connect)
            .await
            .map_err(|_| GbxError::Connect {
                addr: addr.to_owned(),
                source: std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out"),
            })??;
        tracing::debug!(addr, "GbxRemote 2 handshake done");
        Ok(Self::from_halves(read, write, options))
    }

    /// Builds a client over an already handshaken byte stream (used by tests and the mock server).
    pub fn from_halves<R, W>(read: R, write: W, options: ClientOptions) -> (GbxClient, Callbacks)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let writer: Writer = FramedWrite::new(Box::new(write), options.codec.clone());
        let inner = Arc::new(Inner {
            writer: tokio::sync::Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            handles: Mutex::new(HandleAllocator::default()),
            call_timeout: options.call_timeout,
            closed: CancellationToken::new(),
        });
        let (callback_tx, callback_rx) = mpsc::channel(options.callback_buffer);
        let reader = FramedRead::new(read, options.codec);
        tokio::spawn(read_loop(reader, Arc::clone(&inner), callback_tx));
        (GbxClient { inner }, callback_rx)
    }

    /// `true` once the connection has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.is_cancelled()
    }

    /// Resolves when the connection closes.
    pub async fn closed(&self) {
        self.inner.closed.cancelled().await;
    }

    /// Calls a method and returns its raw result.
    pub async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, GbxError> {
        if self.is_closed() {
            return Err(GbxError::ConnectionClosed);
        }
        let payload = Bytes::from(xml::encode_call(method, &params));
        let handle = self
            .inner
            .handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next_handle();
        let (tx, rx) = oneshot::channel();
        self.lock_pending().insert(handle, tx);

        tracing::trace!(method, handle, "→ call");
        let sent = self
            .inner
            .writer
            .lock()
            .await
            .send(Frame { handle, payload })
            .await;
        if let Err(err) = sent {
            self.lock_pending().remove(&handle);
            return Err(err.into());
        }

        match tokio::time::timeout(self.inner.call_timeout, rx).await {
            Ok(Ok(Ok(response))) => response.map_err(GbxError::Fault),
            Ok(Ok(Err(xml_error))) => Err(xml_error.into()),
            Ok(Err(_)) => Err(GbxError::ConnectionClosed),
            Err(_) => {
                self.lock_pending().remove(&handle);
                Err(GbxError::Timeout {
                    method: method.to_owned(),
                    timeout: self.inner.call_timeout,
                })
            }
        }
    }

    /// Calls a method and deserializes its result.
    pub async fn call_as<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<T, GbxError> {
        Ok(self.call(method, params).await?.deserialize()?)
    }

    /// Calls a method that returns a boolean and checks it is `true`.
    pub async fn call_ok(&self, method: &str, params: Vec<Value>) -> Result<(), GbxError> {
        match self.call(method, params).await? {
            Value::Bool(true) => Ok(()),
            other => Err(GbxError::UnexpectedResponse {
                method: method.to_owned(),
                expected: "true",
                got: format!("{other:?}"),
            }),
        }
    }

    /// Runs several calls in one round trip (`system.multicall`).
    pub async fn multicall(
        &self,
        calls: Vec<(String, Vec<Value>)>,
    ) -> Result<Vec<Result<Value, Fault>>, GbxError> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }
        let list: Vec<Value> = calls
            .into_iter()
            .map(|(method, params)| {
                let mut s = std::collections::BTreeMap::new();
                s.insert("methodName".to_owned(), Value::String(method));
                s.insert("params".to_owned(), Value::Array(params));
                Value::Struct(s)
            })
            .collect();
        let result = self
            .call("system.multicall", vec![Value::Array(list)])
            .await?;
        let Value::Array(items) = result else {
            return Err(GbxError::UnexpectedResponse {
                method: "system.multicall".to_owned(),
                expected: "array",
                got: result.type_name().to_owned(),
            });
        };
        Ok(items
            .into_iter()
            .map(|item| match item {
                Value::Array(mut single) if !single.is_empty() => Ok(single.swap_remove(0)),
                Value::Struct(fault) => Err(Fault {
                    code: fault.get("faultCode").and_then(Value::as_i32).unwrap_or(0),
                    message: fault
                        .get("faultString")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }),
                other => Ok(other),
            })
            .collect())
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

async fn read_loop<R>(
    mut reader: FramedRead<R, GbxCodec>,
    inner: Arc<Inner>,
    callbacks: mpsc::Sender<Callback>,
) where
    R: AsyncRead + Unpin,
{
    let mut callbacks = Some(callbacks);
    loop {
        let frame = tokio::select! {
            frame = reader.next() => frame,
            () = inner.closed.cancelled() => break,
        };
        let frame = match frame {
            Some(Ok(frame)) => frame,
            Some(Err(err)) => {
                tracing::error!(%err, "GbxRemote read error");
                break;
            }
            None => {
                tracing::info!("GbxRemote connection closed by the server");
                break;
            }
        };
        let text = String::from_utf8_lossy(&frame.payload);
        if frame.is_callback() {
            match xml::decode(&text) {
                Ok(Message::Call(call)) => {
                    tracing::trace!(method = %call.method, "← callback");
                    let callback = Callback::from_call(call);
                    if let Some(tx) = &callbacks {
                        if tx.send(callback).await.is_err() {
                            // Nobody listens to callbacks anymore; keep serving responses.
                            callbacks = None;
                        }
                    }
                }
                Ok(Message::Response(_)) => {
                    tracing::warn!(
                        handle = frame.handle,
                        "ignoring response with a callback handle"
                    );
                }
                Err(err) => tracing::warn!(%err, "could not decode callback"),
            }
        } else {
            let pending = inner
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&frame.handle);
            let Some(pending) = pending else {
                tracing::debug!(
                    handle = frame.handle,
                    "response without pending call (timed out?)"
                );
                continue;
            };
            tracing::trace!(handle = frame.handle, "← response");
            let response = xml::decode_response(&text);
            let _ = pending.send(response);
        }
    }
    inner.closed.cancel();
    // Dropping the senders wakes every pending call with `ConnectionClosed`.
    inner
        .pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}
