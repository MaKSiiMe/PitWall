//! Typed server callbacks.
//!
//! The server sends two families of callbacks:
//! - legacy XML-RPC callbacks (`ManiaPlanet.PlayerConnect`, `ManiaPlanet.PlayerChat`, ...);
//! - mode script callbacks wrapped in `ManiaPlanet.ModeScriptCallbackArray(name, [json])`
//!   (`Trackmania.Event.WayPoint`, `Trackmania.Scores`, `Maniaplanet.StartMap_Start`, ...).
//!
//! Parsing never fails: anything unknown or malformed becomes [`Callback::Unknown`] or
//! [`ScriptCallback::Other`] so the controller keeps running across game updates.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::types::{EntryVal, MapInfo, PlayerInfo, PlayerRanking, ServerStatus};
use crate::value::Value;
use crate::xml::MethodCall;

/// A callback sent by the dedicated server.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Callback {
    PlayerConnect {
        login: String,
        is_spectator: bool,
    },
    PlayerDisconnect {
        login: String,
        reason: String,
    },
    PlayerChat {
        player_uid: i32,
        login: String,
        text: String,
        is_registered_cmd: bool,
    },
    PlayerInfoChanged(PlayerInfo),
    PlayerManialinkPageAnswer {
        player_uid: i32,
        login: String,
        answer: String,
        entries: Vec<EntryVal>,
    },
    PlayerAlliesChanged {
        login: String,
    },
    PlayerIncoherence {
        player_uid: i32,
        login: String,
    },
    BeginMatch,
    EndMatch {
        rankings: Vec<PlayerRanking>,
        winner_team: i32,
    },
    BeginMap(MapInfo),
    EndMap(MapInfo),
    MapListModified {
        current_index: i32,
        next_index: i32,
        is_list_modified: bool,
    },
    StatusChanged(ServerStatus),
    ServerStart,
    ServerStop,
    Echo {
        internal: String,
        public: String,
    },
    VoteUpdated {
        state: String,
        login: String,
        command: String,
        param: String,
    },
    TunnelDataReceived {
        player_uid: i32,
        login: String,
        data: Vec<u8>,
    },
    /// A mode script callback.
    Script(ScriptCallback),
    /// Any other callback, kept raw.
    Unknown {
        method: String,
        #[serde(skip)]
        params: Vec<Value>,
    },
}

/// Mode script callbacks (TM2020 XmlRpc library, JSON payloads).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "callback", rename_all = "snake_case")]
pub enum ScriptCallback {
    Lifecycle(Lifecycle),
    WayPoint(WayPoint),
    Respawn(Respawn),
    StartLine(PlayerEvent),
    GiveUp(PlayerEvent),
    StartCountdown(PlayerEvent),
    Scores(Box<Scores>),
    PointsRepartition {
        points: Vec<i32>,
    },
    WarmUpStart,
    WarmUpEnd,
    WarmUpStartRound(WarmUpRound),
    WarmUpEndRound(WarmUpRound),
    WarmUpStatus(ActiveStatus),
    PauseStatus(ActiveStatus),
    /// Players eliminated in Knockout (`Trackmania.Knockout.Elimination`).
    Elimination {
        account_ids: Vec<String>,
    },
    /// Any other script callback, with its first JSON payload (or the raw strings).
    Other {
        name: String,
        data: serde_json::Value,
    },
}

/// A section of the mode script flow (`Maniaplanet.StartMap_Start`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    StartServer,
    StartMatch,
    StartMap,
    StartRound,
    StartTurn,
    PlayLoop,
    EndTurn,
    EndRound,
    EndMap,
    EndMatch,
    EndServer,
    LoadingMap,
    UnloadingMap,
    Podium,
    ChannelProgression,
}

impl Section {
    fn parse(name: &str) -> Option<Section> {
        Some(match name {
            "StartServer" => Section::StartServer,
            "StartMatch" => Section::StartMatch,
            "StartMap" => Section::StartMap,
            "StartRound" => Section::StartRound,
            "StartTurn" => Section::StartTurn,
            "EndTurn" => Section::EndTurn,
            "EndRound" => Section::EndRound,
            "EndMap" => Section::EndMap,
            "EndMatch" => Section::EndMatch,
            "EndServer" => Section::EndServer,
            "LoadingMap" => Section::LoadingMap,
            "UnloadingMap" => Section::UnloadingMap,
            "Podium" => Section::Podium,
            "ChannelProgression" => Section::ChannelProgression,
            _ => return None,
        })
    }
}

/// Whether a section starts or ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    Start,
    End,
}

/// Payload shared by the flow callbacks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lifecycle {
    pub section: Section,
    pub boundary: Boundary,
    pub count: i32,
    pub time: i32,
    pub restarted: bool,
    pub map: Option<ScriptMap>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LifecyclePayload {
    count: i32,
    time: i32,
    restarted: bool,
    map: Option<ScriptMap>,
}

/// Map description inside script callbacks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptMap {
    pub uid: String,
    pub name: String,
    pub filename: String,
    pub author: String,
    pub authornickname: String,
    pub environment: String,
    pub mood: String,
    pub bronzetime: i32,
    pub silvertime: i32,
    pub goldtime: i32,
    pub authortime: i32,
    pub laprace: bool,
    pub nblaps: i32,
    pub maptype: String,
    pub mapstyle: String,
}

/// `Trackmania.Event.WayPoint`: a player crossed a checkpoint or the finish line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WayPoint {
    pub time: i32,
    pub login: String,
    pub accountid: String,
    pub racetime: i32,
    pub laptime: i32,
    pub checkpointinrace: i32,
    pub checkpointinlap: i32,
    pub isendrace: bool,
    pub isendlap: bool,
    pub isinfinitelaps: bool,
    pub isindependentlaps: bool,
    pub curracecheckpoints: Vec<i32>,
    pub curlapcheckpoints: Vec<i32>,
    pub blockid: String,
    pub speed: f64,
}

/// `Trackmania.Event.Respawn`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Respawn {
    pub time: i32,
    pub login: String,
    pub accountid: String,
    pub nbrespawns: i32,
    pub racetime: i32,
    pub laptime: i32,
    pub checkpointinrace: i32,
    pub checkpointinlap: i32,
    pub speed: f64,
}

/// Script events that only identify a player (`StartLine`, `GiveUp`, `StartCountdown`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerEvent {
    pub time: i32,
    pub login: String,
    pub accountid: String,
}

/// `Trackmania.Scores`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scores {
    pub responseid: String,
    /// `""`, `"PreEndRound"`, `"EndRound"`, `"EndMap"` or `"EndMatch"`.
    pub section: String,
    pub useteams: bool,
    pub winnerteam: i32,
    pub winnerplayer: String,
    pub teams: Vec<TeamScore>,
    pub players: Vec<PlayerScore>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeamScore {
    pub id: i32,
    pub name: String,
    pub roundpoints: i32,
    pub mappoints: i32,
    pub matchpoints: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerScore {
    pub login: String,
    pub accountid: String,
    pub name: String,
    pub team: i32,
    pub rank: i32,
    pub roundpoints: i32,
    pub mappoints: i32,
    pub matchpoints: i32,
    pub bestracetime: i32,
    pub bestracerespawns: i32,
    pub bestracecheckpoints: Vec<i32>,
    pub bestlaptime: i32,
    pub bestlapcheckpoints: Vec<i32>,
    pub prevracetime: i32,
    pub prevracecheckpoints: Vec<i32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WarmUpRound {
    pub current: i32,
    pub total: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActiveStatus {
    pub available: bool,
    pub active: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PointsRepartitionPayload {
    pointsrepartition: Vec<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EliminationPayload {
    accountids: Vec<String>,
}

/// Strips the `ManiaPlanet.` / `TrackMania.` / `Trackmania.` namespace (case-insensitive).
fn strip_namespace(method: &str) -> &str {
    for prefix in ["ManiaPlanet.", "TrackMania.", "Trackmania.", "Maniaplanet."] {
        if let Some(rest) = method.strip_prefix(prefix) {
            return rest;
        }
    }
    let lower = method.to_ascii_lowercase();
    for prefix in ["maniaplanet.", "trackmania."] {
        if lower.starts_with(prefix) {
            return &method[prefix.len()..];
        }
    }
    method
}

struct Params<'a> {
    method: &'a str,
    params: &'a [Value],
}

#[derive(Debug, thiserror::Error)]
#[error("callback {method}: parameter {index} is missing or not a {expected}")]
struct ParamError {
    method: String,
    index: usize,
    expected: &'static str,
}

impl<'a> Params<'a> {
    fn err(&self, index: usize, expected: &'static str) -> ParamError {
        ParamError {
            method: self.method.to_owned(),
            index,
            expected,
        }
    }

    fn string(&self, index: usize) -> Result<String, ParamError> {
        match self.params.get(index) {
            Some(Value::String(s)) => Ok(s.clone()),
            // Optional trailing strings are sometimes omitted.
            None => Ok(String::new()),
            Some(_) => Err(self.err(index, "string")),
        }
    }

    fn int(&self, index: usize) -> Result<i32, ParamError> {
        self.params
            .get(index)
            .and_then(Value::as_i32)
            .ok_or_else(|| self.err(index, "int"))
    }

    fn boolean(&self, index: usize) -> Result<bool, ParamError> {
        self.params
            .get(index)
            .and_then(Value::as_bool)
            .ok_or_else(|| self.err(index, "boolean"))
    }

    fn typed<T: DeserializeOwned>(&self, index: usize) -> Result<T, ParamError> {
        self.params
            .get(index)
            .cloned()
            .ok_or_else(|| self.err(index, "value"))?
            .deserialize()
            .map_err(|_| self.err(index, std::any::type_name::<T>()))
    }
}

impl Callback {
    /// Converts a raw `<methodCall>` from the server into a typed callback.
    pub fn from_call(call: MethodCall) -> Callback {
        match Self::try_from_call(&call) {
            Ok(Some(callback)) => callback,
            Ok(None) => Callback::Unknown {
                method: call.method,
                params: call.params,
            },
            Err(err) => {
                tracing::warn!(%err, "malformed callback, keeping it raw");
                Callback::Unknown {
                    method: call.method,
                    params: call.params,
                }
            }
        }
    }

    fn try_from_call(call: &MethodCall) -> Result<Option<Callback>, ParamError> {
        let p = Params {
            method: &call.method,
            params: &call.params,
        };
        Ok(Some(match strip_namespace(&call.method) {
            "PlayerConnect" => Callback::PlayerConnect {
                login: p.string(0)?,
                is_spectator: p.boolean(1)?,
            },
            "PlayerDisconnect" => Callback::PlayerDisconnect {
                login: p.string(0)?,
                reason: p.string(1)?,
            },
            "PlayerChat" => Callback::PlayerChat {
                player_uid: p.int(0)?,
                login: p.string(1)?,
                text: p.string(2)?,
                is_registered_cmd: p.boolean(3).unwrap_or(false),
            },
            "PlayerInfoChanged" => Callback::PlayerInfoChanged(p.typed(0)?),
            "PlayerManialinkPageAnswer" => Callback::PlayerManialinkPageAnswer {
                player_uid: p.int(0)?,
                login: p.string(1)?,
                answer: p.string(2)?,
                entries: if call.params.len() > 3 {
                    p.typed(3)?
                } else {
                    Vec::new()
                },
            },
            "PlayerAlliesChanged" => Callback::PlayerAlliesChanged {
                login: p.string(0)?,
            },
            "PlayerIncoherence" => Callback::PlayerIncoherence {
                player_uid: p.int(0)?,
                login: p.string(1)?,
            },
            "BeginMatch" => Callback::BeginMatch,
            "EndMatch" => Callback::EndMatch {
                rankings: p.typed(0).unwrap_or_default(),
                winner_team: p.int(1).unwrap_or(-1),
            },
            "BeginMap" | "BeginChallenge" => Callback::BeginMap(p.typed(0)?),
            "EndMap" | "EndChallenge" => Callback::EndMap(p.typed(0)?),
            "MapListModified" | "ChallengeListModified" => Callback::MapListModified {
                current_index: p.int(0)?,
                next_index: p.int(1)?,
                is_list_modified: p.boolean(2)?,
            },
            "StatusChanged" => Callback::StatusChanged(ServerStatus {
                code: p.int(0)?,
                name: p.string(1)?,
            }),
            "ServerStart" => Callback::ServerStart,
            "ServerStop" => Callback::ServerStop,
            "Echo" => Callback::Echo {
                internal: p.string(0)?,
                public: p.string(1)?,
            },
            "VoteUpdated" => Callback::VoteUpdated {
                state: p.string(0)?,
                login: p.string(1)?,
                command: p.string(2)?,
                param: p.string(3)?,
            },
            "TunnelDataReceived" => Callback::TunnelDataReceived {
                player_uid: p.int(0)?,
                login: p.string(1)?,
                data: match call.params.get(2) {
                    Some(Value::Base64(bytes)) => bytes.clone(),
                    _ => return Err(p.err(2, "base64")),
                },
            },
            "ModeScriptCallbackArray" => {
                let name = p.string(0)?;
                let payloads: Vec<String> = match call.params.get(1) {
                    Some(Value::Array(items)) => items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| p.err(1, "string[]"))
                        })
                        .collect::<Result<_, _>>()?,
                    None => Vec::new(),
                    Some(_) => return Err(p.err(1, "array")),
                };
                Callback::Script(ScriptCallback::parse(&name, &payloads))
            }
            "ModeScriptCallback" => {
                let name = p.string(0)?;
                let payload = p.string(1)?;
                Callback::Script(ScriptCallback::parse(&name, &[payload]))
            }
            _ => return Ok(None),
        }))
    }
}

impl ScriptCallback {
    /// Parses a script callback from its name and string parameters.
    pub fn parse(name: &str, payloads: &[String]) -> ScriptCallback {
        match Self::try_parse(name, payloads) {
            Ok(Some(cb)) => cb,
            Ok(None) => Self::other(name, payloads),
            Err(err) => {
                tracing::warn!(callback = name, %err, "malformed script callback, keeping it raw");
                Self::other(name, payloads)
            }
        }
    }

    fn other(name: &str, payloads: &[String]) -> ScriptCallback {
        let data = payloads
            .first()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_else(|| serde_json::Value::from(payloads.to_vec()));
        ScriptCallback::Other {
            name: name.to_owned(),
            data,
        }
    }

    fn try_parse(
        name: &str,
        payloads: &[String],
    ) -> Result<Option<ScriptCallback>, serde_json::Error> {
        fn json<T: DeserializeOwned + Default>(
            payloads: &[String],
        ) -> Result<T, serde_json::Error> {
            match payloads.first() {
                Some(p) if !p.trim().is_empty() => serde_json::from_str(p),
                _ => Ok(T::default()),
            }
        }

        let short = strip_namespace(name);
        // Flow callbacks: `Maniaplanet.<Section>_Start|_End`, plus `StartPlayLoop` / `EndPlayLoop`.
        let lifecycle =
            |section: Section, boundary: Boundary| -> Result<ScriptCallback, serde_json::Error> {
                let payload: LifecyclePayload = json(payloads)?;
                Ok(ScriptCallback::Lifecycle(Lifecycle {
                    section,
                    boundary,
                    count: payload.count,
                    time: payload.time,
                    restarted: payload.restarted,
                    map: payload.map,
                }))
            };
        if short == "StartPlayLoop" {
            return lifecycle(Section::PlayLoop, Boundary::Start).map(Some);
        }
        if short == "EndPlayLoop" {
            return lifecycle(Section::PlayLoop, Boundary::End).map(Some);
        }
        for (suffix, boundary) in [("_Start", Boundary::Start), ("_End", Boundary::End)] {
            if let Some(section) = short.strip_suffix(suffix).and_then(Section::parse) {
                return lifecycle(section, boundary).map(Some);
            }
        }

        Ok(Some(match short {
            "Event.WayPoint" => ScriptCallback::WayPoint(json(payloads)?),
            "Event.Respawn" => ScriptCallback::Respawn(json(payloads)?),
            "Event.StartLine" => ScriptCallback::StartLine(json(payloads)?),
            "Event.GiveUp" => ScriptCallback::GiveUp(json(payloads)?),
            "Event.StartCountdown" => ScriptCallback::StartCountdown(json(payloads)?),
            "Scores" => ScriptCallback::Scores(Box::new(json(payloads)?)),
            "PointsRepartition" => {
                let payload: PointsRepartitionPayload = json(payloads)?;
                ScriptCallback::PointsRepartition {
                    points: payload.pointsrepartition,
                }
            }
            "WarmUp.Start" => ScriptCallback::WarmUpStart,
            "WarmUp.End" => ScriptCallback::WarmUpEnd,
            "WarmUp.StartRound" => ScriptCallback::WarmUpStartRound(json(payloads)?),
            "WarmUp.EndRound" => ScriptCallback::WarmUpEndRound(json(payloads)?),
            "WarmUp.Status" => ScriptCallback::WarmUpStatus(json(payloads)?),
            "Pause.Status" => ScriptCallback::PauseStatus(json(payloads)?),
            "Knockout.Elimination" => {
                let payload: EliminationPayload = json(payloads)?;
                ScriptCallback::Elimination {
                    account_ids: payload.accountids,
                }
            }
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::decode_call;

    fn parse(xml: &str) -> Callback {
        Callback::from_call(decode_call(xml).unwrap())
    }

    #[test]
    fn player_connect() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.PlayerConnect</methodName><params>\
             <param><value><string>abc</string></value></param>\
             <param><value><boolean>1</boolean></value></param></params></methodCall>",
        );
        assert_eq!(
            cb,
            Callback::PlayerConnect {
                login: "abc".into(),
                is_spectator: true
            }
        );
    }

    #[test]
    fn player_chat() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.PlayerChat</methodName><params>\
             <param><value><int>236</int></value></param>\
             <param><value><string>abc</string></value></param>\
             <param><value><string>/mode cup</string></value></param>\
             <param><value><boolean>0</boolean></value></param>\
             <param><value><int>0</int></value></param></params></methodCall>",
        );
        assert_eq!(
            cb,
            Callback::PlayerChat {
                player_uid: 236,
                login: "abc".into(),
                text: "/mode cup".into(),
                is_registered_cmd: false
            }
        );
    }

    #[test]
    fn manialink_answer_with_entries() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.PlayerManialinkPageAnswer</methodName><params>\
             <param><value><int>5</int></value></param>\
             <param><value><string>abc</string></value></param>\
             <param><value><string>pw:toggle:ranking</string></value></param>\
             <param><value><array><data><value><struct>\
             <member><name>Name</name><value><string>search</string></value></member>\
             <member><name>Value</name><value><string>A01</string></value></member>\
             </struct></value></data></array></value></param></params></methodCall>",
        );
        let Callback::PlayerManialinkPageAnswer {
            answer, entries, ..
        } = cb
        else {
            panic!("unexpected {cb:?}");
        };
        assert_eq!(answer, "pw:toggle:ranking");
        assert_eq!(entries[0].name, "search");
        assert_eq!(entries[0].value, "A01");
    }

    #[test]
    fn begin_map_with_legacy_struct() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.BeginMap</methodName><params><param><value><struct>\
             <member><name>Uid</name><value><string>u1d</string></value></member>\
             <member><name>Name</name><value><string>Spring 2026 - 01</string></value></member>\
             <member><name>NbCheckpoints</name><value><int>4</int></value></member>\
             </struct></value></param></params></methodCall>",
        );
        let Callback::BeginMap(map) = cb else {
            panic!("unexpected {cb:?}");
        };
        assert_eq!(map.uid, "u1d");
        assert_eq!(map.nb_checkpoints, 4);
    }

    fn script(name: &str, json: &str) -> ScriptCallback {
        ScriptCallback::parse(name, &[json.to_owned()])
    }

    #[test]
    fn waypoint_from_documented_payload() {
        let cb = script(
            "Trackmania.Event.WayPoint",
            r##"{"time": 123456, "login": "PlayerLogin", "accountid": "0b1c", "racetime": 123456,
                "laptime": 45678, "stuntsscore": 3457, "checkpointinrace": 3, "checkpointinlap": 0,
                "isendrace": false, "isendlap": false, "curracecheckpoints": [1234, 5200, 7580, 9000],
                "curlapcheckpoints": [1420], "blockid": "#123", "speed": 456.45, "distance": 398.49}"##,
        );
        let ScriptCallback::WayPoint(wp) = cb else {
            panic!("unexpected {cb:?}");
        };
        assert_eq!(wp.login, "PlayerLogin");
        assert_eq!(wp.racetime, 123_456);
        assert_eq!(wp.checkpointinrace, 3);
        assert_eq!(wp.curracecheckpoints, vec![1234, 5200, 7580, 9000]);
        assert!(!wp.isendrace);
    }

    #[test]
    fn scores_from_documented_payload() {
        let cb = script(
            "Trackmania.Scores",
            r#"{"responseid": "xyz", "section": "EndRound", "useteams": true, "winnerteam": 1,
                "winnerplayer": "PlayerLogin1",
                "teams": [{"id": 0, "name": "blue", "roundpoints": 498, "mappoints": 46, "matchpoints": 9}],
                "players": [{"login": "PlayerLogin1", "name": "Player#1", "rank": 1, "roundpoints": 456,
                  "mappoints": 345, "matchpoints": 64, "bestracetime": 456789, "bestracerespawns": 2,
                  "bestracecheckpoints": [1230, 7546, 19045, 456789], "bestlaptime": 68942,
                  "bestlaprespawns": 1, "bestlapcheckpoints": [2458, 9874], "stuntsscore": 492}]}"#,
        );
        let ScriptCallback::Scores(scores) = cb else {
            panic!("unexpected {cb:?}");
        };
        assert_eq!(scores.section, "EndRound");
        assert_eq!(scores.teams[0].mappoints, 46);
        assert_eq!(scores.players[0].bestracetime, 456_789);
    }

    #[test]
    fn lifecycle_callbacks() {
        let cb = script(
            "Maniaplanet.StartMap_Start",
            r#"{"count": 2, "restarted": true, "time": 10, "map": {"uid": "u", "name": "n"}}"#,
        );
        assert_eq!(
            cb,
            ScriptCallback::Lifecycle(Lifecycle {
                section: Section::StartMap,
                boundary: Boundary::Start,
                count: 2,
                time: 10,
                restarted: true,
                map: Some(ScriptMap {
                    uid: "u".into(),
                    name: "n".into(),
                    ..ScriptMap::default()
                }),
            })
        );
        let cb = script("Maniaplanet.EndPlayLoop", r#"{"count": 1, "time": 5}"#);
        assert!(matches!(
            cb,
            ScriptCallback::Lifecycle(Lifecycle {
                section: Section::PlayLoop,
                boundary: Boundary::End,
                ..
            })
        ));
    }

    #[test]
    fn warmup_and_pause() {
        assert_eq!(
            script("Trackmania.WarmUp.Start", "{}"),
            ScriptCallback::WarmUpStart
        );
        assert_eq!(
            script(
                "Trackmania.WarmUp.StartRound",
                r#"{"current": 2, "total": 3}"#
            ),
            ScriptCallback::WarmUpStartRound(WarmUpRound {
                current: 2,
                total: 3
            })
        );
        assert_eq!(
            script(
                "Maniaplanet.Pause.Status",
                r#"{"responseid": "", "available": true, "active": false}"#
            ),
            ScriptCallback::PauseStatus(ActiveStatus {
                available: true,
                active: false
            })
        );
    }

    #[test]
    fn unknown_and_malformed_script_callbacks_are_kept() {
        let cb = script("Some.Custom.Callback", r#"{"a": 1}"#);
        assert_eq!(
            cb,
            ScriptCallback::Other {
                name: "Some.Custom.Callback".into(),
                data: serde_json::json!({"a": 1})
            }
        );
        let cb = script("Trackmania.Event.WayPoint", "not json");
        assert!(matches!(cb, ScriptCallback::Other { .. }));
    }

    #[test]
    fn mode_script_callback_array_is_unwrapped() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.ModeScriptCallbackArray</methodName><params>\
             <param><value><string>Trackmania.Event.GiveUp</string></value></param>\
             <param><value><array><data><value><string>{&quot;login&quot;: &quot;abc&quot;, &quot;time&quot;: 1}</string></value></data></array></value></param>\
             </params></methodCall>",
        );
        assert_eq!(
            cb,
            Callback::Script(ScriptCallback::GiveUp(PlayerEvent {
                time: 1,
                login: "abc".into(),
                accountid: String::new()
            }))
        );
    }

    #[test]
    fn unknown_callbacks_stay_raw() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.BillUpdated</methodName><params>\
             <param><value><int>1</int></value></param></params></methodCall>",
        );
        assert!(
            matches!(cb, Callback::Unknown { method, .. } if method == "ManiaPlanet.BillUpdated")
        );
    }

    #[test]
    fn malformed_legacy_callbacks_stay_raw() {
        let cb = parse(
            "<methodCall><methodName>ManiaPlanet.PlayerConnect</methodName><params>\
             <param><value><int>1</int></value></param></params></methodCall>",
        );
        assert!(matches!(cb, Callback::Unknown { .. }));
    }
}
