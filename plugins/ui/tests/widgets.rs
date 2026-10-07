//! UI plugin against the mock server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::mock::{MockServer, default_handler};
use pitwall_core::gbx::xml::MethodCall;
use pitwall_core::{
    Controller, ControllerOptions, Event, Lang, RoleBook, RolesConfig, ServerConfig, Translator,
};
use pitwall_formats::FormatRegistry;
use pitwall_plugin_ui::UiPlugin;
use tokio_util::sync::CancellationToken;

async fn mock() -> MockServer {
    MockServer::with_handler(|call: &MethodCall| match call.method.as_str() {
        "GetDetailedPlayerInfo" => Ok(Value::Struct(
            [
                ("Login".to_owned(), call.params[0].clone()),
                ("NickName".to_owned(), Value::from("$0afNick")),
                ("Language".to_owned(), Value::from("fr")),
            ]
            .into_iter()
            .collect(),
        )),
        _ => default_handler(call),
    })
    .await
    .unwrap()
}

/// Manialink XML sent to `login` so far.
fn pages_for(server: &MockServer, login: &str) -> Vec<String> {
    server
        .calls()
        .into_iter()
        .filter(|c| {
            c.method == "SendDisplayManialinkPageToLogin"
                && c.params[0]
                    .as_str()
                    .is_some_and(|l| l.split(',').any(|x| x == login))
        })
        .filter_map(|c| c.params[1].as_str().map(str::to_owned))
        .collect()
}

async fn wait_until(what: &str, f: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn widgets_follow_the_race_and_preferences() {
    let server = mock().await;
    let data_dir = std::env::temp_dir().join(format!("pitwall-ui-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
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
    controller.add_plugin(UiPlugin::new(
        Arc::new(RwLock::new(FormatRegistry::builtin().unwrap())),
        data_dir.clone(),
    ));
    let handles = controller.handles();
    let mut events = handles.bus.raw_subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(controller.run(shutdown.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(events.recv().await, Ok(Event::Connected { .. })) {}
    })
    .await
    .unwrap();

    // A French player joins: views in French, no admin panel.
    server.send_callback(
        "ManiaPlanet.PlayerConnect",
        vec!["bob".into(), false.into()],
    );
    wait_until("views", || {
        pages_for(&server, "bob")
            .iter()
            .any(|p| p.contains("id=\"PitWall_Ranking\""))
    })
    .await;
    let pages = pages_for(&server, "bob");
    assert!(pages.iter().any(|p| p.contains("CLASSEMENT LIVE")));
    assert!(pages.iter().any(|p| p.contains("id=\"PitWall_Menu\"")));
    assert!(
        !pages
            .iter()
            .any(|p| p.contains("id=\"PitWall_AdminPanel\""))
    );

    // A finish updates the ranking data layer (targeted to bob).
    server.send_script_callback(
        "Trackmania.Event.WayPoint",
        &serde_json::json!({"login": "bob", "racetime": 42_123, "isendrace": true, "curracecheckpoints": [10000, 42123]}),
    );
    wait_until("ranking data", || {
        pages_for(&server, "bob")
            .iter()
            .any(|p| p.contains("PitWall_Ranking_Data") && p.contains("42123"))
    })
    .await;
    assert!(
        pages_for(&server, "bob")
            .iter()
            .any(|p| p.contains("PitWall_CpDiff_Data") && p.contains("10000"))
    );

    // bob hides the records widget from the menu.
    server.send_callback(
        "ManiaPlanet.PlayerManialinkPageAnswer",
        vec![
            1.into(),
            "bob".into(),
            "pw:ui:toggle:records".into(),
            Value::Array(vec![]),
        ],
    );
    wait_until("records hidden", || {
        pages_for(&server, "bob")
            .iter()
            .any(|p| p.trim() == r#"<manialink id="PitWall_Records" version="3"></manialink>"#)
    })
    .await;
    let saved = std::fs::read_to_string(data_dir.join("ui_prefs.json")).unwrap();
    assert!(saved.contains("records"));

    // A forged admin action from a player is refused by the command registry.
    server.send_callback(
        "ManiaPlanet.PlayerManialinkPageAnswer",
        vec![
            1.into(),
            "bob".into(),
            "pw:cmd:role bob superadmin".into(),
            Value::Array(vec![]),
        ],
    );
    wait_until("permission denied", || {
        server.calls().iter().any(|c| {
            c.method == "ChatSendServerMessageToLogin"
                && c.params[0]
                    .as_str()
                    .is_some_and(|t| t.contains("pas le droit"))
        })
    })
    .await;
    assert_eq!(handles.roles.get("bob"), pitwall_core::Role::Player);
    shutdown.cancel();
}
