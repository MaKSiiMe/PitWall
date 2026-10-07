//! Local map upload against the mock server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::mock::MockServer;
use pitwall_core::{
    Actor, Controller, ControllerOptions, Event, Lang, ReplyTo, Role, RoleBook, RolesConfig,
    ServerConfig, Translator,
};
use pitwall_integrations::http_client;
use pitwall_integrations::tmx::Tmx;
use pitwall_plugin_maps::MapsPlugin;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn uploads_a_local_map_and_rejects_paths() {
    let server = MockServer::start().await.unwrap();
    let dir = std::env::temp_dir().join(format!("pitwall-maps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Test.Map.Gbx"), b"GBX\x06\x00fake map").unwrap();

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
        dir.clone(),
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

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    handles.invoke(
        Actor::Console,
        Role::SuperAdmin,
        Lang::En,
        "map local",
        ReplyTo::Channel(tx.clone()),
    );
    let listing = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(listing.contains("Test.Map.Gbx"), "{listing}");

    handles.invoke(
        Actor::Console,
        Role::SuperAdmin,
        Lang::En,
        "map local Test.Map.Gbx",
        ReplyTo::Channel(tx.clone()),
    );
    let added = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(added.contains("Map added"), "{added}");
    let calls = server.calls();
    let write = calls.iter().find(|c| c.method == "WriteFile").unwrap();
    assert_eq!(write.params[0], Value::from("PitWall/Test.Map.Gbx"));
    assert!(
        calls
            .iter()
            .any(|c| c.method == "AddMap" && c.params[0] == Value::from("PitWall/Test.Map.Gbx"))
    );

    handles.invoke(
        Actor::Console,
        Role::SuperAdmin,
        Lang::En,
        "map local ../../etc/passwd",
        ReplyTo::Channel(tx),
    );
    let refused = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(refused.contains("Map error"), "{refused}");
    shutdown.cancel();
}
