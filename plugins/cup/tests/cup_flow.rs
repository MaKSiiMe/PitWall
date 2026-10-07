//! Full cup flow with a controller and the mock server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::mock::{MockServer, default_handler};
use pitwall_core::gbx::xml::MethodCall;
use pitwall_core::{
    Actor, Controller, ControllerOptions, Event, Lang, Role, RoleBook, RolesConfig, ServerConfig,
    Translator,
};
use pitwall_formats::FormatRegistry;
use pitwall_formats::cup::{CupInput, CupLibrary, CupStatus};
use pitwall_plugin_cup::CupPlugin;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pitwall-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Mock with two connected players and a server-like mode switch.
async fn mock() -> MockServer {
    let scripts = Arc::new(Mutex::new((
        "Trackmania/TM_TimeAttack_Online.Script.txt".to_owned(),
        String::new(),
    )));
    MockServer::with_handler(move |call: &MethodCall| {
        let mut st = scripts.lock().unwrap();
        let s = |pairs: Vec<(&str, Value)>| {
            Value::Struct(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
        };
        match call.method.as_str() {
            "WriteFile" => {
                if let Some(Value::Base64(data)) = call.params.get(1) {
                    st.1 = String::from_utf8_lossy(data).into_owned();
                }
                Ok(true.into())
            }
            "RestartMap" | "NextMap" => {
                if let Some(start) = st.1.find("<script_name>") {
                    let rest = st.1[start + 13..].to_owned();
                    if let Some(end) = rest.find("</script_name>") {
                        st.0 = format!("{}.Script.txt", &rest[..end]);
                    }
                }
                Ok(true.into())
            }
            "GetScriptName" => Ok(s(vec![
                ("CurrentValue", st.0.clone().into()),
                ("NextValue", st.0.clone().into()),
            ])),
            "GetMapList" => Ok(Value::Array(vec![s(vec![(
                "FileName",
                "A.Map.Gbx".into(),
            )])])),
            "GetCurrentMapIndex" => Ok(0.into()),
            "GetPlayerList" => Ok(Value::Array(
                ["alice", "bob", "carol"]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        s(vec![
                            ("Login", (*l).into()),
                            ("NickName", l.to_uppercase().into()),
                            ("PlayerId", i32::try_from(i + 1).unwrap().into()),
                        ])
                    })
                    .collect(),
            )),
            _ => default_handler(call),
        }
    })
    .await
    .unwrap()
}

async fn wait_for(events: &mut broadcast::Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if pred(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event did not arrive")
}

async fn wait_until(what: &str, f: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn runs_a_two_phase_cup() {
    let server = mock().await;
    let cups_dir = temp_dir("cups");
    let data_dir = temp_dir("data");
    std::fs::write(
        cups_dir.join("mini.toml"),
        r#"
id = "mini"
participants = ["alice", "bob"]

[[phase]]
id = "quali"
format = "timeattack"
qualify = { top = 1 }

[[phase]]
id = "final"
format = "cup"
"#,
    )
    .unwrap();

    let formats = FormatRegistry::builtin().unwrap();
    let library = CupLibrary::load(&cups_dir, &formats).unwrap();
    let (plugin, handle) = CupPlugin::new(
        Arc::new(RwLock::new(formats)),
        Arc::new(RwLock::new(library)),
        data_dir.clone(),
    );

    let (host, port) = server
        .addr()
        .rsplit_once(':')
        .map(|(h, p)| (h.to_owned(), p.parse().unwrap()))
        .unwrap();
    let mut controller = Controller::new(
        ControllerOptions {
            server: ServerConfig {
                host,
                port,
                password: "x".into(),
                ..ServerConfig::default()
            },
            default_lang: Lang::En,
            ..ControllerOptions::default()
        },
        RoleBook::from_config(&RolesConfig::default()),
        Translator::with_core_catalogs().unwrap(),
    );
    controller.add_plugin(plugin);
    let handles = controller.handles();
    let mut events = handles.bus.raw_subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(controller.run(shutdown.clone()));
    wait_for(&mut events, |e| matches!(e, Event::Connected { .. })).await;

    // Load and start from the console.
    handles.invoke(
        Actor::Console,
        Role::SuperAdmin,
        Lang::En,
        "cup load mini",
        pitwall_core::ReplyTo::Nowhere,
    );
    wait_until("cup loaded", || handle.snapshot().is_some()).await;
    handle
        .input(CupInput::Start { connected: vec![] }, Actor::Console)
        .await
        .unwrap();
    assert_eq!(
        handle.snapshot().unwrap().status,
        CupStatus::WaitingStart { phase: 0 }
    );

    // carol is not registered: forced to spectate; alice and bob race.
    wait_until("placement", || {
        server.calls().iter().any(|c| {
            c.method == "ForceSpectator"
                && c.params[0] == Value::from("carol")
                && c.params[1] == Value::Int(1)
        })
    })
    .await;

    handle.input(CupInput::Go, Actor::Console).await.unwrap();
    wait_until("format applied", || {
        server
            .method_names()
            .contains(&"LoadMatchSettings".to_owned())
    })
    .await;
    assert_eq!(
        handle.snapshot().unwrap().status,
        CupStatus::Loading { phase: 0 }
    );

    // Map reload with the new mode → running.
    server.send_script_callback(
        "Maniaplanet.StartMap_Start",
        &serde_json::json!({"count": 1}),
    );
    wait_until("running", || {
        handle.snapshot().unwrap().status == CupStatus::Running { phase: 0 }
    })
    .await;

    // End of the qualifier: bob first.
    server.send_script_callback(
        "Trackmania.Scores",
        &serde_json::json!({"section": "EndMatch", "players": [
            {"login": "bob", "rank": 1, "bestracetime": 40000},
            {"login": "alice", "rank": 2, "bestracetime": 41000}
        ]}),
    );
    let event = wait_for(
        &mut events,
        |e| matches!(e, Event::Custom { name, .. } if name == "phase_completed"),
    )
    .await;
    let Event::Custom { payload, .. } = event else {
        unreachable!()
    };
    assert_eq!(payload["result"]["qualified"], serde_json::json!(["bob"]));
    let snapshot = handle.snapshot().unwrap();
    assert_eq!(snapshot.status, CupStatus::WaitingStart { phase: 1 });
    assert_eq!(snapshot.racing, ["bob"]);

    // The state is on disk for crash recovery.
    let saved = std::fs::read_to_string(data_dir.join("cups/active.json")).unwrap();
    assert!(saved.contains("\"waiting_start\""));

    // Chat announcements were sent.
    assert!(
        server
            .method_names()
            .iter()
            .any(|m| m == "ChatSendServerMessageToLogin")
    );
    shutdown.cancel();
}
