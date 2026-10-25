//! Replays a recording ([`crate::record`]) on the [`MockServer`]: the server answers PitWall's
//! calls with the recorded responses and pushes the recorded callbacks, in order.
//!
//! Responses are matched by method, in recording order: the n-th `GetPlayerList` gets the n-th
//! recorded answer, then the last one again. Methods never recorded fall back to
//! [`default_handler`]. Callbacks are replayed in order, with the recorded gaps divided by a
//! speed factor (and capped), so a session of minutes replays in a test in a few seconds.
//!
//! [`anonymize`] replaces logins, account ids, nicknames and chat text, so a recording can
//! become a test in the (public) repository.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;

use crate::frame::CLIENT_HANDLE_START;
use crate::mock::{MockServer, default_handler};
use crate::record::{Direction, Entry};
use crate::value::Value;
use crate::xml::{self, Message, MethodCall, MethodResponse};

/// Errors reading a recording.
#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("line {line}: {source}")]
    Json {
        line: usize,
        source: serde_json::Error,
    },
}

/// A recorded session.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// Method → recorded responses, in order.
    responses: HashMap<String, Vec<MethodResponse>>,
    /// Callbacks (XML) with their time, in order.
    callbacks: Vec<(u64, String)>,
}

impl Trace {
    pub fn load(path: &Path) -> Result<Trace, TraceError> {
        let text = std::fs::read_to_string(path).map_err(|source| TraceError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Trace::parse(&text)
    }

    /// Parses JSON lines. Frames without content (undecodable when recorded) are skipped.
    pub fn parse(text: &str) -> Result<Trace, TraceError> {
        let mut trace = Trace::default();
        // Handle → method of the calls waiting for their response.
        let mut pending: HashMap<u32, String> = HashMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry = serde_json::from_str(line).map_err(|source| TraceError::Json {
                line: index + 1,
                source,
            })?;
            if entry.event.is_some() {
                pending.clear();
                continue;
            }
            let (Some(dir), Some(handle), Some(xml_text)) = (entry.dir, entry.handle, entry.xml)
            else {
                continue;
            };
            match (dir, xml::decode(&xml_text)) {
                (Direction::Out, Ok(Message::Call(call))) => {
                    pending.insert(handle, call.method);
                }
                (Direction::In, Ok(Message::Call(_))) if handle < CLIENT_HANDLE_START => {
                    trace.callbacks.push((entry.t, xml_text));
                }
                (Direction::In, Ok(Message::Response(response))) => {
                    if let Some(method) = pending.remove(&handle) {
                        trace.responses.entry(method).or_default().push(response);
                    }
                }
                _ => {}
            }
        }
        Ok(trace)
    }

    /// Number of recorded callbacks.
    pub fn callback_count(&self) -> usize {
        self.callbacks.len()
    }

    /// A [`MockServer`] handler answering with the recorded responses.
    pub fn handler(&self) -> impl Fn(&MethodCall) -> MethodResponse + Send + Sync + 'static {
        let queues: HashMap<String, (VecDeque<MethodResponse>, Option<MethodResponse>)> = self
            .responses
            .iter()
            .map(|(method, responses)| {
                (method.clone(), (responses.iter().cloned().collect(), None))
            })
            .collect();
        let queues = Arc::new(Mutex::new(queues));
        move |call: &MethodCall| {
            let mut queues = queues.lock().unwrap_or_else(PoisonError::into_inner);
            match queues.get_mut(&call.method) {
                Some((queue, last)) => match queue.pop_front() {
                    Some(response) => {
                        *last = Some(response.clone());
                        response
                    }
                    None => last.clone().unwrap_or_else(|| default_handler(call)),
                },
                None => default_handler(call),
            }
        }
    }

    /// Pushes every callback to the server's clients, keeping the recorded gaps divided by
    /// `speed` and capped at `max_gap`.
    pub async fn play(&self, server: &MockServer, speed: f64, max_gap: Duration) {
        let mut previous: Option<u64> = None;
        for (t, callback) in &self.callbacks {
            if let Some(previous) = previous {
                let gap = Duration::from_millis(t.saturating_sub(previous))
                    .div_f64(speed.max(f64::MIN_POSITIVE))
                    .min(max_gap);
                if !gap.is_zero() {
                    tokio::time::sleep(gap).await;
                }
            }
            previous = Some(*t);
            server.send_raw_callback(callback.clone());
        }
    }
}

/// Anonymizes a recording (JSON lines): logins and account ids become stable fake ones,
/// nicknames become `Player N`, chat text that is not a command becomes `…`.
pub fn anonymize(text: &str) -> Result<String, TraceError> {
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Entry = serde_json::from_str(line).map_err(|source| TraceError::Json {
            line: index + 1,
            source,
        })?;
        entries.push(entry);
    }

    // Collect what identifies people.
    let mut logins = BTreeMap::new();
    let mut nicknames = BTreeMap::new();
    for entry in &entries {
        let Some(text) = &entry.xml else { continue };
        for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
            if is_login(token) && !is_fake_login(token) {
                let n = logins.len() + 1;
                logins
                    .entry(token.to_owned())
                    .or_insert_with(|| fake_login(n));
            }
        }
        if let Ok(message) = xml::decode(text) {
            let values: Vec<Value> = match message {
                Message::Call(call) => call.params,
                Message::Response(Ok(value)) => vec![value],
                Message::Response(Err(_)) => vec![],
            };
            for value in &values {
                collect_nicknames(value, &mut nicknames);
            }
        }
    }
    let nicknames: Vec<(String, String)> = nicknames
        .into_keys()
        .filter(|nick| !is_fake_nickname(nick))
        .enumerate()
        .map(|(i, nick)| (nick, format!("Player {}", i + 1)))
        .collect();
    let mut replacements: Vec<(String, String)> = Vec::new();
    for (login, fake) in &logins {
        if let (Some(real_id), Some(fake_id)) = (account_id(login), account_id(fake)) {
            replacements.push((real_id, fake_id));
        }
        replacements.push((login.clone(), fake.clone()));
    }
    for (nick, fake) in nicknames {
        // Nicknames appear as-is in JSON and XML-escaped in XML.
        replacements.push((xml_escape(&nick), fake.clone()));
        replacements.push((json_escape(&nick), fake.clone()));
        replacements.push((nick, fake));
    }
    // Longest first, so a nickname containing another is replaced whole; equal ones adjacent.
    replacements.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    replacements.dedup_by(|a, b| a.0 == b.0);

    let mut out = String::new();
    for mut entry in entries {
        if let Some(text) = entry.xml.take() {
            let text = mute_chat(&text);
            let text = replacements
                .iter()
                .filter(|(from, _)| !from.is_empty())
                .fold(text, |text, (from, to)| text.replace(from.as_str(), to));
            entry.xml = Some(text);
        }
        let line =
            serde_json::to_string(&entry).map_err(|source| TraceError::Json { line: 0, source })?;
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// A TM2020 login: 16 bytes in unpadded base64url (22 characters).
fn is_login(token: &str) -> bool {
    token.len() == 22
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .is_ok_and(|b| b.len() == 16)
}

/// Fake logins and nicknames are kept, so anonymizing twice changes nothing.
fn is_fake_login(login: &str) -> bool {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(login)
        .is_ok_and(|b| b.starts_with(b"pitwall-"))
}

fn is_fake_nickname(nick: &str) -> bool {
    nick.strip_prefix("Player ")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn fake_login(n: usize) -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(b"pitwall-");
    bytes[8..].copy_from_slice(&(n as u64).to_be_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn account_id(login: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(login)
        .ok()?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        hex.get(0..8)?,
        hex.get(8..12)?,
        hex.get(12..16)?,
        hex.get(16..20)?,
        hex.get(20..32)?
    ))
}

/// Nicknames from `NickName` struct members and from `name` next to `login` in script
/// callbacks' JSON.
fn collect_nicknames(value: &Value, out: &mut BTreeMap<String, ()>) {
    match value {
        Value::Struct(members) => {
            for (key, member) in members {
                if key.eq_ignore_ascii_case("nickname")
                    && let Value::String(nick) = member
                    && !nick.is_empty()
                {
                    out.insert(nick.clone(), ());
                }
                collect_nicknames(member, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_nicknames(i, out)),
        Value::String(text) if text.trim_start().starts_with('{') => {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
                collect_json_nicknames(&json, out);
            }
        }
        _ => {}
    }
}

fn collect_json_nicknames(json: &serde_json::Value, out: &mut BTreeMap<String, ()>) {
    match json {
        serde_json::Value::Object(map) => {
            if map.contains_key("login")
                && let Some(serde_json::Value::String(name)) = map.get("name")
                && !name.is_empty()
            {
                out.insert(name.clone(), ());
            }
            map.values().for_each(|v| collect_json_nicknames(v, out));
        }
        serde_json::Value::Array(items) => {
            items.iter().for_each(|v| collect_json_nicknames(v, out))
        }
        _ => {}
    }
}

/// Replaces the text of chat callbacks that are not commands.
fn mute_chat(text: &str) -> String {
    let Ok(Message::Call(mut call)) = xml::decode(text) else {
        return text.to_owned();
    };
    if call.method != "ManiaPlanet.PlayerChat" {
        return text.to_owned();
    }
    if let Some(Value::String(message)) = call.params.get_mut(2)
        && !message.trim_start().starts_with('/')
    {
        *message = "…".to_owned();
    }
    xml::encode_call(&call.method, &call.params)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn json_escape(text: &str) -> String {
    let quoted = serde_json::Value::String(text.to_owned()).to_string();
    quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or(text)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Entry;

    fn line(t: u64, dir: Direction, handle: u32, xml: String) -> String {
        serde_json::to_string(&Entry {
            t,
            dir: Some(dir),
            handle: Some(handle),
            xml: Some(xml),
            event: None,
        })
        .unwrap()
    }

    const BOB: &str = "L9H2ZIXkRCmJytSeTSCDoQ";

    fn sample() -> String {
        [
            line(
                1,
                Direction::Out,
                0x8000_0001,
                xml::encode_call("GetPlayerList", &[]),
            ),
            line(
                2,
                Direction::In,
                0x8000_0001,
                xml::encode_response(&Ok(Value::Array(vec![Value::Struct(
                    [
                        ("Login".to_owned(), Value::from(BOB)),
                        ("NickName".to_owned(), Value::from("$f00Bob & co")),
                    ]
                    .into_iter()
                    .collect(),
                )]))),
            ),
            line(
                3,
                Direction::In,
                1,
                xml::encode_call(
                    "ManiaPlanet.PlayerChat",
                    &[1.into(), BOB.into(), "salut tout le monde".into(), false.into()],
                ),
            ),
            line(
                4,
                Direction::In,
                2,
                xml::encode_call(
                    "ManiaPlanet.ModeScriptCallbackArray",
                    &[
                        "Trackmania.Event.WayPoint".into(),
                        Value::Array(vec![
                            serde_json::json!({"login": BOB, "accountid": "2fd1f664-85e4-4429-89ca-d49e4d2083a1", "racetime": 12345})
                                .to_string()
                                .into(),
                        ]),
                    ],
                ),
            ),
        ]
        .join("\n")
    }

    #[test]
    fn parses_responses_by_method_and_callbacks_in_order() {
        let trace = Trace::parse(&sample()).unwrap();
        assert_eq!(trace.callback_count(), 2);
        let handler = trace.handler();
        let call = |method: &str| MethodCall {
            method: method.into(),
            params: vec![],
        };
        let first = handler(&call("GetPlayerList")).unwrap();
        assert!(format!("{first:?}").contains(BOB));
        // Exhausted: the last answer again.
        assert_eq!(handler(&call("GetPlayerList")).unwrap(), first);
        // Never recorded: the default mock answer.
        assert_eq!(
            handler(&call("GetMapsDirectory")).unwrap(),
            Value::from("/server/UserData/Maps/")
        );
    }

    #[test]
    fn anonymizes_logins_nicknames_and_chat() {
        let out = anonymize(&sample()).unwrap();
        assert!(!out.contains(BOB), "{out}");
        assert!(
            !out.contains("2fd1f664-85e4-4429-89ca-d49e4d2083a1"),
            "{out}"
        );
        assert!(!out.contains("Bob"), "{out}");
        assert!(!out.contains("salut"), "{out}");
        assert!(
            out.contains("Player 1") && out.contains(&fake_login(1)),
            "{out}"
        );
        // Still a valid trace, with consistent fake ids.
        let trace = Trace::parse(&out).unwrap();
        assert_eq!(trace.callback_count(), 2);
        assert!(out.contains(&account_id(&fake_login(1)).unwrap()));
        assert_eq!(
            anonymize(&out).unwrap(),
            out,
            "anonymizing twice changes nothing"
        );
    }
}
