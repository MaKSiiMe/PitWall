//! Playlist commands (`/map move|next|play|remove|clear`) against a mock with a real playlist.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::mock::{MockPlaylist, MockServer, playlist_handler};
use pitwall_core::{
    Actor, Controller, ControllerOptions, Event, Handles, Lang, ReplyTo, Role, RoleBook,
    RolesConfig, ServerConfig, Translator,
};
use pitwall_integrations::http_client;
use pitwall_integrations::tmx::Tmx;
use pitwall_plugin_maps::MapsPlugin;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn files(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

struct Setup {
    server: MockServer,
    playlist: Arc<Mutex<MockPlaylist>>,
    handles: Handles,
    shutdown: CancellationToken,
}

async fn setup() -> Setup {
    let playlist = Arc::new(Mutex::new(MockPlaylist {
        maps: files(&["a", "b", "c", "d"]),
        playing: "b".to_owned(),
    }));
    let server = MockServer::with_handler(playlist_handler(Arc::clone(&playlist)))
        .await
        .unwrap();
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
    controller.add_plugin(MapsPlugin::new(
        Tmx::new(http_client().unwrap()),
        None,
        None,
        std::env::temp_dir(),
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
    // The runtime learns the current map from the map start.
    server.send_script_callback(
        "Maniaplanet.StartMap_Start",
        &serde_json::json!({"count": 1, "restarted": false, "map": {"uid": "uid-b"}}),
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(events.recv().await, Ok(Event::MapStart { .. })) {}
    })
    .await
    .unwrap();
    Setup {
        server,
        playlist,
        handles,
        shutdown,
    }
}

/// Runs a console command and returns its first answer.
async fn run(handles: &Handles, line: &str) -> String {
    let (tx, mut rx) = mpsc::unbounded_channel();
    handles.invoke(
        Actor::Console,
        Role::SuperAdmin,
        Lang::En,
        line,
        ReplyTo::Channel(tx),
    );
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn playlist_commands() {
    let s = setup().await;
    let maps = || s.playlist.lock().unwrap().maps.clone();

    let answer = run(&s.handles, "map list").await;
    assert!(answer.contains("1. "), "{answer}");

    let answer = run(&s.handles, "map move 4 1").await;
    assert!(answer.contains("moved"), "{answer}");
    assert_eq!(maps(), files(&["d", "a", "b", "c"]));

    let answer = run(&s.handles, "map move 9 1").await;
    assert!(answer.contains("Map error"), "{answer}");
    assert_eq!(maps(), files(&["d", "a", "b", "c"]));

    let answer = run(&s.handles, "map next 4").await;
    assert!(answer.contains("played next"), "{answer}");
    assert!(
        s.server
            .calls()
            .iter()
            .any(|c| c.method == "ChooseNextMap" && c.params[0] == Value::from("c"))
    );

    run(&s.handles, "map remove 1").await;
    assert_eq!(maps(), files(&["a", "b", "c"]));

    let answer = run(&s.handles, "map clear").await;
    assert!(answer.contains("cleared"), "{answer}");
    assert_eq!(maps(), files(&["b"]));
    s.shutdown.cancel();
}
