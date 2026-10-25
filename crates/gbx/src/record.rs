//! Recording of the raw GbxRemote traffic, to turn real sessions into tests.
//!
//! Every frame, in both directions, becomes one JSON line:
//!
//! ```json
//! {"t":1729771234567,"dir":"out","handle":2147483649,"xml":"<?xml …><methodCall>…"}
//! {"t":1729771234570,"dir":"in","handle":2147483649,"xml":"<?xml …><methodResponse>…"}
//! {"t":1729771234600,"event":"connected"}
//! ```
//!
//! `dir` is `out` for frames sent by PitWall (calls) and `in` for frames from the server
//! (responses, and callbacks: handles below `0x8000_0000`). Secrets are masked before writing:
//! the parameters of the password methods (`Authenticate`, `SetServerPassword`…), the results of
//! the password getters, every struct member whose name contains `password`, and `/password …`
//! typed in the chat. A frame that cannot be decoded is recorded without its content.
//!
//! Writing happens on a dedicated thread: the connection never waits for the disk, and frames
//! are dropped (and counted) if the thread falls behind. Files are `gbx-<unix ms>.jsonl` in one
//! directory; a new file starts past `max_file_bytes`, and the oldest go past `max_files`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::frame::CLIENT_HANDLE_START;
use crate::value::Value;
use crate::xml::{self, Message, MethodCall, MethodResponse};

/// What replaces a secret.
pub const MASK: &str = "***";

/// Methods whose parameters are all secrets.
const SECRET_PARAMS: &[&str] = &[
    "Authenticate",
    "ChangeAuthPassword",
    "SetServerPassword",
    "SetServerPasswordForSpectator",
    "SetRefereePassword",
];

/// Methods whose result is a secret.
const SECRET_RESULTS: &[&str] = &[
    "GetServerPassword",
    "GetServerPasswordForSpectator",
    "GetRefereePassword",
];

/// Frames waiting for the writer thread before new ones are dropped.
const QUEUE: usize = 16 * 1024;

/// Where and how much to record.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub dir: PathBuf,
    /// A new file starts once the current one is larger.
    pub max_file_bytes: u64,
    /// Files kept, the newest; older ones are deleted.
    pub max_files: usize,
}

/// One line of a recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix time, milliseconds.
    pub t: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<Direction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<u32>,
    /// The frame's XML, secrets masked; `None` if it could not be decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xml: Option<String>,
    /// `connected` or `disconnected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
}

/// Direction of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Sent by PitWall.
    Out,
    /// Sent by the server.
    In,
}

/// Work for the writer thread.
enum Job {
    Frame {
        t: u64,
        dir: Direction,
        handle: u32,
        payload: Vec<u8>,
    },
    Event {
        t: u64,
        name: &'static str,
    },
}

/// Handle to the recording thread. Cheap to clone.
#[derive(Clone)]
pub struct Recorder {
    tx: SyncSender<Job>,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Recorder {
    /// Creates the directory and starts the writer thread.
    pub fn start(config: RecorderConfig) -> std::io::Result<Recorder> {
        fs::create_dir_all(&config.dir)?;
        let (tx, rx) = sync_channel(QUEUE);
        std::thread::Builder::new()
            .name("gbx-recorder".to_owned())
            .spawn(move || Writer::new(config).run(rx))?;
        Ok(Recorder {
            tx,
            dropped: Arc::default(),
        })
    }

    /// Records a frame sent to the server.
    pub fn sent(&self, handle: u32, payload: &[u8]) {
        self.frame(Direction::Out, handle, payload);
    }

    /// Records a frame received from the server.
    pub fn received(&self, handle: u32, payload: &[u8]) {
        self.frame(Direction::In, handle, payload);
    }

    /// Records a connection event (`connected`, `disconnected`).
    pub fn event(&self, name: &'static str) {
        self.push(Job::Event { t: now_ms(), name });
    }

    fn frame(&self, dir: Direction, handle: u32, payload: &[u8]) {
        self.push(Job::Frame {
            t: now_ms(),
            dir,
            handle,
            payload: payload.to_vec(),
        });
    }

    fn push(&self, message: Job) {
        match self.tx.try_send(message) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if dropped.is_power_of_two() {
                    tracing::warn!(dropped, "recording falls behind, frames dropped");
                }
            }
            // The thread stopped after an error it logged.
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// Methods of a call waiting for its response, to mask secret results.
enum Pending {
    Single(String),
    Multi(Vec<String>),
}

struct Writer {
    config: RecorderConfig,
    file: Option<(BufWriter<File>, u64)>,
    pending: HashMap<u32, Pending>,
}

impl Writer {
    fn new(config: RecorderConfig) -> Writer {
        Writer {
            config,
            file: None,
            pending: HashMap::new(),
        }
    }

    fn run(mut self, rx: Receiver<Job>) {
        loop {
            // Flush when idle so a crash loses little.
            let message = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => {
                    self.flush();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return self.flush(),
            };
            let entry = match message {
                Job::Frame {
                    t,
                    dir,
                    handle,
                    payload,
                } => Entry {
                    t,
                    dir: Some(dir),
                    handle: Some(handle),
                    xml: self.redact(dir, handle, &payload),
                    event: None,
                },
                Job::Event { t, name } => {
                    self.pending.clear();
                    Entry {
                        t,
                        dir: None,
                        handle: None,
                        xml: None,
                        event: Some(name.to_owned()),
                    }
                }
            };
            if let Err(err) = self.write(&entry) {
                tracing::error!(%err, dir = %self.config.dir.display(), "recording stopped");
                return;
            }
        }
    }

    fn flush(&mut self) {
        if let Some((file, _)) = &mut self.file
            && let Err(err) = file.flush()
        {
            tracing::warn!(%err, "cannot flush the recording");
        }
    }

    fn redact(&mut self, dir: Direction, handle: u32, payload: &[u8]) -> Option<String> {
        let text = String::from_utf8_lossy(payload);
        match (dir, xml::decode(&text).ok()?) {
            (Direction::Out, Message::Call(mut call)) => {
                self.pending.insert(handle, pending_of(&call));
                redact_call(&mut call);
                Some(xml::encode_call(&call.method, &call.params))
            }
            (Direction::In, Message::Call(mut call)) if handle < CLIENT_HANDLE_START => {
                redact_call(&mut call);
                Some(xml::encode_call(&call.method, &call.params))
            }
            (Direction::In, Message::Response(mut response)) => {
                if let Some(pending) = self.pending.remove(&handle) {
                    redact_response(&pending, &mut response);
                }
                if let Ok(value) = &mut response {
                    redact_value(value);
                }
                Some(xml::encode_response(&response))
            }
            _ => None,
        }
    }

    fn write(&mut self, entry: &Entry) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let full = self
            .file
            .as_ref()
            .is_none_or(|(_, size)| *size >= self.config.max_file_bytes);
        if full {
            self.rotate()?;
        }
        if let Some((file, size)) = &mut self.file {
            file.write_all(&line)?;
            *size += line.len() as u64;
        }
        Ok(())
    }

    /// Starts a new file and deletes the oldest ones.
    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some((mut file, _)) = self.file.take() {
            file.flush()?;
        }
        let t = now_ms();
        // Two rotations in the same millisecond get a suffix.
        let path = (0..)
            .map(|n| match n {
                0 => self.config.dir.join(format!("gbx-{t}.jsonl")),
                n => self.config.dir.join(format!("gbx-{t}-{n}.jsonl")),
            })
            .find(|p| !p.exists())
            .unwrap_or_else(|| self.config.dir.join(format!("gbx-{t}.jsonl")));
        self.file = Some((BufWriter::new(File::create(&path)?), 0));
        tracing::info!(path = %path.display(), "recording GbxRemote traffic");
        let mut files = recordings(&self.config.dir)?;
        while files.len() > self.config.max_files.max(1) {
            let oldest = files.remove(0);
            if let Err(err) = fs::remove_file(&oldest) {
                tracing::warn!(%err, path = %oldest.display(), "cannot delete an old recording");
            }
        }
        Ok(())
    }
}

/// Recordings of a directory, oldest first.
pub fn recordings(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("gbx-") && n.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    Ok(files)
}

fn pending_of(call: &MethodCall) -> Pending {
    if call.method == "system.multicall" {
        let methods = match call.params.first() {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::Struct(s) => s
                        .get("methodName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    _ => String::new(),
                })
                .collect(),
            _ => Vec::new(),
        };
        Pending::Multi(methods)
    } else {
        Pending::Single(call.method.clone())
    }
}

/// Masks the secrets of a call (or a callback).
pub fn redact_call(call: &mut MethodCall) {
    redact_params(&call.method, &mut call.params);
    if call.method == "system.multicall"
        && let Some(Value::Array(items)) = call.params.first_mut()
    {
        for item in items {
            if let Value::Struct(s) = item {
                let method = s
                    .get("methodName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if let Some(Value::Array(params)) = s.get_mut("params") {
                    redact_params(&method, params);
                }
            }
        }
    }
    // `/password secret` typed in the chat.
    if call.method == "ManiaPlanet.PlayerChat"
        && let Some(Value::String(text)) = call.params.get_mut(2)
    {
        let command = text.trim_start().trim_start_matches('/');
        if command
            .get(..8)
            .is_some_and(|c| c.eq_ignore_ascii_case("password"))
        {
            *text = format!("/password {MASK}");
        }
    }
}

fn redact_params(method: &str, params: &mut [Value]) {
    if SECRET_PARAMS.contains(&method) {
        for param in params.iter_mut() {
            *param = Value::String(MASK.to_owned());
        }
    }
    for param in params {
        redact_value(param);
    }
}

fn redact_response(pending: &Pending, response: &mut MethodResponse) {
    let Ok(value) = response else { return };
    match pending {
        Pending::Single(method) if SECRET_RESULTS.contains(&method.as_str()) => {
            *value = Value::String(MASK.to_owned());
        }
        Pending::Multi(methods) => {
            if let Value::Array(items) = value {
                for (item, method) in items.iter_mut().zip(methods) {
                    if SECRET_RESULTS.contains(&method.as_str())
                        && let Value::Array(single) = item
                    {
                        for v in single {
                            *v = Value::String(MASK.to_owned());
                        }
                    }
                }
            }
        }
        Pending::Single(_) => {}
    }
}

/// Masks every struct member whose name contains `password`, at any depth.
pub fn redact_value(value: &mut Value) {
    match value {
        Value::Struct(members) => {
            for (key, member) in members.iter_mut() {
                if key.to_ascii_lowercase().contains("password") {
                    *member = Value::String(MASK.to_owned());
                } else {
                    redact_value(member);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pairs: &[(&str, Value)]) -> Value {
        Value::Struct(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn masks_password_methods_and_members() {
        let mut call = MethodCall {
            method: "Authenticate".into(),
            params: vec!["SuperAdmin".into(), "hunter2".into()],
        };
        redact_call(&mut call);
        assert_eq!(call.params, vec![Value::from(MASK), Value::from(MASK)]);

        let mut options = MethodCall {
            method: "SetServerOptions".into(),
            params: vec![s(&[
                ("Name", "PitWall".into()),
                ("Password", "secret".into()),
                ("PasswordForSpectator", "spec".into()),
                ("RefereePassword", "ref".into()),
            ])],
        };
        redact_call(&mut options);
        let text = format!("{:?}", options.params);
        assert!(text.contains("PitWall"));
        assert!(!text.contains("secret") && !text.contains("spec\"") && !text.contains("ref\""));
    }

    #[test]
    fn masks_secrets_inside_multicalls() {
        let multicall = |method: &str, param: &str| MethodCall {
            method: "system.multicall".into(),
            params: vec![Value::Array(vec![s(&[
                ("methodName", method.into()),
                ("params", Value::Array(vec![param.into()])),
            ])])],
        };
        let mut call = multicall("SetServerPassword", "secret");
        redact_call(&mut call);
        assert!(!format!("{call:?}").contains("secret"));
        let mut call = multicall("ChatSendServerMessage", "hello");
        redact_call(&mut call);
        assert!(format!("{call:?}").contains("hello"));

        let mut response: MethodResponse = Ok(Value::Array(vec![
            Value::Array(vec!["secret".into()]),
            Value::Array(vec!["Mock map".into()]),
        ]));
        redact_response(
            &Pending::Multi(vec!["GetServerPassword".into(), "GetMapInfo".into()]),
            &mut response,
        );
        let text = format!("{response:?}");
        assert!(!text.contains("secret") && text.contains("Mock map"));
    }

    #[test]
    fn masks_password_results_and_chat() {
        let mut response: MethodResponse = Ok("secret".into());
        redact_response(&Pending::Single("GetServerPassword".into()), &mut response);
        assert_eq!(response, Ok(Value::from(MASK)));

        for typed in ["/password abc", "//PASSWORD abc", "/password"] {
            let mut chat = MethodCall {
                method: "ManiaPlanet.PlayerChat".into(),
                params: vec![1.into(), "bob".into(), typed.into(), false.into()],
            };
            redact_call(&mut chat);
            assert_eq!(chat.params[2], Value::from(format!("/password {MASK}")));
        }
        let mut chat = MethodCall {
            method: "ManiaPlanet.PlayerChat".into(),
            params: vec![1.into(), "bob".into(), "/mode rounds".into(), false.into()],
        };
        redact_call(&mut chat);
        assert_eq!(chat.params[2], Value::from("/mode rounds"));
    }
}
