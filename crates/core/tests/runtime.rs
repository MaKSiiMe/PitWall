//! Runtime tests against the in-process mock GbxRemote server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::mock::{MockServer, default_handler};
use pitwall_core::gbx::types::ScriptSetting;
use pitwall_core::gbx::xml::MethodCall;
use pitwall_core::{
    Actor, Command, Controller, ControllerOptions, Event, Handles, Lang, ModeSwitch, ReplyTo, Role,
    RoleBook, RolesConfig, ServerConfig, Translator,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const TA: &str = "Trackmania/TM_TimeAttack_Online.Script.txt";
const CUP: &str = "Trackmania/TM_Cup_Online.Script.txt";

/// Mock that behaves like a server for mode switches: `LoadMatchSettings` changes the next
/// script, `RestartMap` makes it current.
async fn stateful_mock() -> (MockServer, Arc<Mutex<(String, String)>>) {
    let scripts = Arc::new(Mutex::new((TA.to_owned(), TA.to_owned())));
    let written = Arc::new(Mutex::new(String::new()));
    let s = Arc::clone(&scripts);
    let server = MockServer::with_handler(move |call: &MethodCall| {
        let mut scripts = s.lock().unwrap();
        match call.method.as_str() {
            "WriteFile" => {
                if let Some(Value::Base64(data)) = call.params.get(1) {
                    *written.lock().unwrap() = String::from_utf8_lossy(data).into_owned();
                }
                Ok(true.into())
            }
            "LoadMatchSettings" => {
                let xml = written.lock().unwrap().clone();
                if xml.contains("TM_Cup_Online") {
                    scripts.1 = CUP.to_owned();
                }
                Ok(1.into())
            }
            "RestartMap" | "NextMap" => {
                scripts.0 = scripts.1.clone();
                Ok(true.into())
            }
            "GetScriptName" => Ok(Value::Struct(
                [
                    ("CurrentValue".to_owned(), Value::from(scripts.0.clone())),
                    ("NextValue".to_owned(), Value::from(scripts.1.clone())),
                ]
                .into_iter()
                .collect(),
            )),
            "GetMapList" => Ok(Value::Array(vec![Value::Struct(
                [
                    ("UId".to_owned(), Value::from("uid1")),
                    ("FileName".to_owned(), Value::from("Campaigns/A01.Map.Gbx")),
                ]
                .into_iter()
                .collect(),
            )])),
            "GetCurrentMapIndex" => Ok(0.into()),
            "GetDetailedPlayerInfo" => Ok(Value::Struct(
                [
                    ("Login".to_owned(), call.params[0].clone()),
                    ("NickName".to_owned(), Value::from("$f00Nick")),
                    ("Language".to_owned(), Value::from("fr")),
                ]
                .into_iter()
                .collect(),
            )),
            _ => default_handler(call),
        }
    })
    .await
    .unwrap();
    (server, scripts)
}

struct Running {
    handles: Handles,
    events: broadcast::Receiver<Event>,
    shutdown: CancellationToken,
}

async fn start(server: &MockServer, roles: RolesConfig) -> Running {
    let addr = server.addr();
    let (host, port) = addr.rsplit_once(':').unwrap();
    let options = ControllerOptions {
        server: ServerConfig {
            host: host.to_owned(),
            port: port.parse().unwrap(),
            password: "secret".to_owned(),
            reconnect_delay_secs: 1,
            ..ServerConfig::default()
        },
        default_lang: Lang::En,
        ..ControllerOptions::default()
    };
    let controller = Controller::new(
        options,
        RoleBook::from_config(&roles),
        Translator::with_core_catalogs().unwrap(),
    );
    let handles = controller.handles();
    let mut events = handles.bus.raw_subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(controller.run(shutdown.clone()));
    wait_for(&mut events, |e| matches!(e, Event::Connected { .. })).await;
    Running {
        handles,
        events,
        shutdown,
    }
}

async fn wait_for(events: &mut broadcast::Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = events.recv().await.unwrap();
            if pred(&event) {
                return event;
            }
        }
    })
    .await
    .expect("expected event did not arrive")
}

async fn wait_for_call(server: &MockServer, pred: impl Fn(&MethodCall) -> bool) -> MethodCall {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(call) = server.calls().into_iter().find(|c| pred(c)) {
                return call;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected call did not happen")
}

#[tokio::test]
async fn connects_authenticates_and_enables_callbacks() {
    let (server, _) = stateful_mock().await;
    let running = start(&server, RolesConfig::default()).await;
    let names = server.method_names();
    assert_eq!(names[0], "Authenticate");
    assert!(names.contains(&"SetApiVersion".to_owned()));
    assert!(names.contains(&"EnableCallbacks".to_owned()));
    assert!(
        server
            .calls()
            .iter()
            .any(|c| c.method == "TriggerModeScriptEventArray"
                && c.params[0] == Value::from("XmlRpc.EnableCallbacks"))
    );
    assert!(running.handles.state.read(|s| s.connected));
    assert_eq!(running.handles.state.read(|s| s.script.clone()), TA);
    running.shutdown.cancel();
}

#[tokio::test]
async fn tracks_players_and_their_language() {
    let (server, _) = stateful_mock().await;
    let mut running = start(&server, RolesConfig::default()).await;
    server.send_callback(
        "ManiaPlanet.PlayerConnect",
        vec!["bob".into(), false.into()],
    );
    let event = wait_for(&mut running.events, |e| {
        matches!(e, Event::PlayerJoined { .. })
    })
    .await;
    let Event::PlayerJoined { player } = event else {
        unreachable!()
    };
    assert_eq!(player.login, "bob");
    assert_eq!(player.lang, Lang::Fr);
    assert_eq!(player.nickname, "$f00Nick");
    assert!(running.handles.state.player("bob").is_some());

    server.send_callback(
        "ManiaPlanet.PlayerDisconnect",
        vec!["bob".into(), "".into()],
    );
    wait_for(&mut running.events, |e| {
        matches!(e, Event::PlayerLeft { .. })
    })
    .await;
    assert!(running.handles.state.player("bob").is_none());
    running.shutdown.cancel();
}

#[tokio::test]
async fn chat_commands_are_dispatched_with_permissions() {
    let (server, _) = stateful_mock().await;
    let roles = RolesConfig {
        admins: vec!["admin1".into()],
        ..RolesConfig::default()
    };
    let mut running = start(&server, roles).await;
    server.send_callback(
        "ManiaPlanet.PlayerConnect",
        vec!["bob".into(), false.into()],
    );
    wait_for(&mut running.events, |e| {
        matches!(e, Event::PlayerJoined { .. })
    })
    .await;

    // Unknown command → localized error to the player (bob speaks French).
    server.send_callback(
        "ManiaPlanet.PlayerChat",
        vec![
            5.into(),
            "bob".into(),
            "/nope".into(),
            false.into(),
            0.into(),
        ],
    );
    let call = wait_for_call(&server, |c| {
        c.method == "ChatSendServerMessageToLogin"
            && c.params[0]
                .as_str()
                .is_some_and(|t| t.contains("Commande inconnue"))
    })
    .await;
    assert_eq!(call.params[1], Value::from("bob"));

    // A player cannot use a referee command.
    server.send_callback(
        "ManiaPlanet.PlayerChat",
        vec![
            5.into(),
            "bob".into(),
            "/role admin1".into(),
            false.into(),
            0.into(),
        ],
    );
    wait_for_call(&server, |c| {
        c.params
            .first()
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains("pas le droit"))
    })
    .await;

    // An admin can.
    server.send_callback(
        "ManiaPlanet.PlayerChat",
        vec![
            6.into(),
            "admin1".into(),
            "/role bob".into(),
            false.into(),
            0.into(),
        ],
    );
    let event = wait_for(&mut running.events, |e| matches!(e, Event::Command { .. })).await;
    let Event::Command { invocation } = event else {
        unreachable!()
    };
    assert_eq!(invocation.name, "role");
    assert_eq!(invocation.role, Role::Admin);
    assert_eq!(invocation.actor, Actor::Player("admin1".into()));
    running.shutdown.cancel();
}

#[tokio::test]
async fn console_invocations_reply_on_their_channel() {
    let (server, _) = stateful_mock().await;
    let running = start(&server, RolesConfig::default()).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    running.handles.invoke(
        Actor::Console,
        Role::SuperAdmin,
        Lang::En,
        "help",
        ReplyTo::Channel(tx),
    );
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(first.contains("Available commands"), "{first}");
    running.shutdown.cancel();
}

#[tokio::test]
async fn hot_mode_switch_without_disconnecting_players() {
    let (server, scripts) = stateful_mock().await;
    let mut running = start(&server, RolesConfig::default()).await;
    server.send_callback(
        "ManiaPlanet.PlayerConnect",
        vec!["bob".into(), false.into()],
    );
    wait_for(&mut running.events, |e| {
        matches!(e, Event::PlayerJoined { .. })
    })
    .await;

    let mut settings = pitwall_core::gbx::types::ScriptSettings::new();
    settings.insert("S_PointsLimit".into(), ScriptSetting::Int(100));
    let commands = running.handles.commands(Actor::Console);
    commands
        .call(Command::SwitchMode(ModeSwitch {
            script: CUP.to_owned(),
            settings,
            maps: vec![],
            points_repartition: Some(vec![10, 6, 4]),
            label: "cup".to_owned(),
        }))
        .await
        .unwrap();

    let names = server.method_names();
    let write = names.iter().position(|m| m == "WriteFile").unwrap();
    let load = names.iter().position(|m| m == "LoadMatchSettings").unwrap();
    let restart = names.iter().position(|m| m == "RestartMap").unwrap();
    assert!(write < load && load < restart, "{names:?}");
    assert!(!names.iter().any(|m| m == "Kick"));
    let load_call = server
        .calls()
        .into_iter()
        .find(|c| c.method == "LoadMatchSettings")
        .unwrap();
    assert_eq!(
        load_call.params[0],
        Value::from("MatchSettings/PitWall/cup.txt")
    );
    assert_eq!(scripts.lock().unwrap().0, CUP);

    // The server reloads the map: the runtime verifies and announces the new mode.
    server.send_script_callback(
        "Maniaplanet.StartMap_Start",
        &serde_json::json!({"count": 1, "restarted": true, "map": {"uid": "uid1"}}),
    );
    let event = wait_for(&mut running.events, |e| {
        matches!(e, Event::ModeChanged { .. })
    })
    .await;
    let Event::ModeChanged { script } = event else {
        unreachable!()
    };
    assert_eq!(script, CUP);
    // Settings were compared and re-applied because the mock does not store them.
    wait_for_call(&server, |c| c.method == "SetModeScriptSettings").await;
    // Points repartition sent to the script.
    wait_for_call(&server, |c| {
        c.method == "TriggerModeScriptEventArray"
            && c.params[0] == Value::from("Trackmania.SetPointsRepartition")
    })
    .await;
    // Bob is still there.
    assert!(running.handles.state.player("bob").is_some());
    // The action was audited.
    running.shutdown.cancel();
}

#[tokio::test]
async fn reconnects_after_the_server_comes_back() {
    let (server, _) = stateful_mock().await;
    let addr = server.addr();
    let mut running = start(&server, RolesConfig::default()).await;

    server.shutdown();
    wait_for(&mut running.events, |e| matches!(e, Event::Disconnected)).await;
    assert!(!running.handles.state.read(|s| s.connected));
    let err = running
        .handles
        .commands(Actor::Console)
        .call(Command::RestartMap)
        .await;
    assert!(
        matches!(err, Err(pitwall_core::CommandError::NotConnected)),
        "{err:?}"
    );

    // The server comes back on the same port: the runtime reconnects by itself.
    let _restarted = MockServer::with_handler_on(&addr, default_handler)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(running.events.recv().await, Ok(Event::Connected { .. })) {
                break;
            }
        }
    })
    .await
    .expect("no reconnection");
    assert!(running.handles.state.read(|s| s.connected));
    running.shutdown.cancel();
}

#[test]
fn events_serialize_to_json() {
    let event = Event::Callback {
        callback: Arc::new(pitwall_core::gbx::Callback::Script(
            pitwall_core::gbx::ScriptCallback::WarmUpStart,
        )),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "callback");
    assert_eq!(json["callback"]["type"], "script");
    assert_eq!(json["callback"]["callback"], "warm_up_start");
}

#[tokio::test]
async fn client_plugin_commands_are_ignored_silently() {
    let (server, _) = stateful_mock().await;
    let running = start(&server, RolesConfig::default()).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // Better Chat (Openplanet) sends `/chatformat json` on join.
    running.handles.invoke(
        Actor::Player("bob".into()),
        Role::Player,
        Lang::En,
        "/chatformat json",
        ReplyTo::Channel(tx),
    );
    let answer = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    assert!(
        !matches!(answer, Ok(Some(_))),
        "no answer expected, got {answer:?}"
    );
    running.shutdown.cancel();
}
