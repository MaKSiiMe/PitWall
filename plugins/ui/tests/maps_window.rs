//! Map manager window (`/maps`) against a mock with a real playlist.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::mock::{MockPlaylist, MockServer, playlist_handler};
use pitwall_core::gbx::xml::MethodCall;
use pitwall_core::{
    Actor, Command, Controller, ControllerOptions, Event, Lang, ReplyTo, Role, RoleBook,
    RolesConfig, ServerConfig, Translator,
};
use pitwall_formats::FormatRegistry;
use pitwall_plugin_ui::UiPlugin;
use tokio_util::sync::CancellationToken;

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

/// Last map window sent to `login`.
fn window_for(server: &MockServer, login: &str) -> Option<String> {
    pages_for(server, login)
        .into_iter()
        .rev()
        .find(|p| p.contains("id=\"PitWall_Maps\""))
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

fn click(server: &MockServer, login: &str, action: &str) {
    server.send_callback(
        "ManiaPlanet.PlayerManialinkPageAnswer",
        vec![1.into(), login.into(), action.into(), Value::Array(vec![])],
    );
}

#[tokio::test]
async fn map_window_follows_the_playlist() {
    let playlist = Arc::new(Mutex::new(MockPlaylist {
        maps: ["Alpha", "Bravo", "Charlie"].map(str::to_owned).to_vec(),
        playing: "Bravo".to_owned(),
    }));
    let maps = playlist_handler(Arc::clone(&playlist));
    let server = MockServer::with_handler(move |call: &MethodCall| match call.method.as_str() {
        "GetDetailedPlayerInfo" => Ok(Value::Struct(
            [
                ("Login".to_owned(), call.params[0].clone()),
                ("NickName".to_owned(), Value::from("Nick")),
                ("Language".to_owned(), Value::from("fr")),
            ]
            .into_iter()
            .collect(),
        )),
        _ => maps(call),
    })
    .await
    .unwrap();
    let (host, port) = server
        .addr()
        .rsplit_once(':')
        .map(|(h, p)| (h.to_owned(), p.parse().unwrap()))
        .unwrap();
    let data_dir = std::env::temp_dir().join(format!("pitwall-ui-maps-{}", std::process::id()));
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
        RoleBook::from_config(&RolesConfig {
            admins: vec!["boss".into()],
            ..RolesConfig::default()
        }),
        Translator::with_core_catalogs().unwrap(),
    );
    controller.add_plugin(UiPlugin::new(
        Arc::new(RwLock::new(FormatRegistry::builtin().unwrap())),
        data_dir,
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
    for login in ["boss", "bob"] {
        server.send_callback(
            "ManiaPlanet.PlayerConnect",
            vec![login.into(), false.into()],
        );
    }
    wait_until("players", || {
        handles.state.player("boss").is_some() && handles.state.player("bob").is_some()
    })
    .await;

    // The ranking header shows the mode and the map position, in the player's language.
    server.send_script_callback(
        "Maniaplanet.StartMap_Start",
        &serde_json::json!({"count": 1, "restarted": false, "map": {"uid": "uid-Bravo"}}),
    );
    wait_until("ranking header", || {
        pages_for(&server, "boss").iter().any(|p| {
            p.contains("PitWall_Ranking_Data")
                && p.contains("MAP 2/3")
                && p.contains("CONTRE-LA-MONTRE")
        })
    })
    .await;

    // PitWall's scores table replaces Nadeo's, with every connected player.
    assert!(server.calls().iter().any(|c| {
        c.method == "TriggerModeScriptEventArray"
            && c.params[0] == Value::from("Common.UIModules.SetProperties")
            && format!("{:?}", c.params[1]).contains("Race_ScoresTable")
    }));
    wait_until("scoreboard data", || {
        pages_for(&server, "bob").iter().any(|p| {
            p.contains("PitWall_Scoreboard_Data")
                && p.contains("2 JOUEURS")
                && p.contains("MAP 2/3")
        })
    })
    .await;

    // The admin opens the window: the playlist, in French.
    handles.invoke(
        Actor::Player("boss".into()),
        Role::Admin,
        Lang::Fr,
        "/maps",
        ReplyTo::Player("boss".into()),
    );
    wait_until("map window", || window_for(&server, "boss").is_some()).await;
    let xml = window_for(&server, "boss").unwrap();
    assert!(
        xml.contains("MAPS · 3") && xml.contains("Vider la playlist"),
        "{xml}"
    );

    // A playlist change (from anyone) refreshes it.
    handles
        .commands(Actor::Console)
        .call(Command::SetPlaylist {
            files: ["Charlie", "Alpha", "Bravo"].map(str::to_owned).to_vec(),
        })
        .await
        .unwrap();
    wait_until("refresh after the change", || {
        window_for(&server, "boss").is_some_and(|xml| {
            xml.find("text=\"Charlie\"").unwrap() < xml.find("text=\"Alpha\"").unwrap()
        })
    })
    .await;

    // TMX search results from the maps plugin land in the window.
    handles.bus.publish(Event::custom(
        "maps",
        "search",
        serde_json::json!({"login": "boss", "query": "snow", "results": [
            {"id": 42, "name": "Snow run", "author": "someone"}
        ]}),
    ));
    wait_until("search results", || {
        window_for(&server, "boss").is_some_and(|xml| xml.contains("pw:cmd:map tmx 42 next"))
    })
    .await;

    // A player without the window cannot get it by forging a page action.
    click(&server, "bob", "pw:maps:page:1");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(window_for(&server, "bob").is_none());

    click(&server, "boss", "pw:maps:close");
    wait_until("window closed", || {
        pages_for(&server, "boss")
            .last()
            .is_some_and(|p| p.trim() == r#"<manialink id="PitWall_Maps" version="3"></manialink>"#)
    })
    .await;
    shutdown.cancel();
}
