//! Storage plugin against the mock server and an in-memory database.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use pitwall_core::gbx::mock::MockServer;
use pitwall_core::{
    Actor, Command, Controller, ControllerOptions, Event, Lang, RoleBook, RolesConfig,
    ServerConfig, Translator,
};
use pitwall_plugin_storage::StoragePlugin;
use pitwall_storage::Storage;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn records_runs_audit_and_publishes_best_times() {
    let server = MockServer::start().await.unwrap();
    let db = Storage::in_memory().await.unwrap();
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
    controller.add_plugin(StoragePlugin::new(db.clone()));
    let handles = controller.handles();
    let mut events = handles.bus.raw_subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(controller.run(shutdown.clone()));
    let wait = |pred: fn(&Event) -> bool| {
        let mut rx = handles.bus.raw_subscribe();
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let e = rx.recv().await.unwrap();
                    if pred(&e) {
                        return e;
                    }
                }
            })
            .await
            .expect("event")
        }
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(events.recv().await, Ok(Event::Connected { .. })) {}
    })
    .await
    .unwrap();

    server.send_callback(
        "ManiaPlanet.PlayerConnect",
        vec!["bob".into(), false.into()],
    );
    let records = wait(
        |e| matches!(e, Event::Custom { name, payload, .. } if name == "records" && payload["rows"].as_array().is_some_and(|r| !r.is_empty())),
    );
    server.send_script_callback(
        "Trackmania.Event.WayPoint",
        &serde_json::json!({"login": "bob", "racetime": 33_333, "isendrace": true, "curracecheckpoints": [11_111, 33_333]}),
    );
    let Event::Custom { payload, .. } = records.await else {
        unreachable!()
    };
    assert_eq!(payload["map_uid"], "mockuid");
    assert_eq!(payload["rows"][0]["time"], 33_333);

    let best = db.best_times("mockuid", 10).await.unwrap();
    assert_eq!(best[0].login, "bob");

    // Admin actions land in the audit log.
    handles
        .commands(Actor::Console)
        .call(Command::RestartMap)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if db
                .audit_log(10)
                .await
                .unwrap()
                .iter()
                .any(|a| a.action == "restart_map" && a.actor == "console")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
}
