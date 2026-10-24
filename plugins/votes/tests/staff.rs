//! `/skip` and `/res` from staff run `/next` and `/restart` at once; from players they call a vote.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use pitwall_core::gbx::mock::MockServer;
use pitwall_core::{
    Actor, Controller, ControllerOptions, Event, Handles, Lang, ReplyTo, Role, RoleBook,
    RolesConfig, ServerConfig, Translator,
};
use pitwall_plugin_admin::AdminPlugin;
use pitwall_plugin_votes::VotesPlugin;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

struct Setup {
    server: MockServer,
    handles: Handles,
    events: broadcast::Receiver<Event>,
    shutdown: CancellationToken,
}

async fn setup() -> Setup {
    let server = MockServer::start().await.unwrap();
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
    // Same order as the binary: admin first, then votes.
    controller.add_plugin(AdminPlugin);
    controller.add_plugin(VotesPlugin);
    let handles = controller.handles();
    let mut events = handles.bus.raw_subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(controller.run(shutdown.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(events.recv().await, Ok(Event::Connected { .. })) {}
    })
    .await
    .unwrap();
    Setup {
        server,
        handles,
        events,
        shutdown,
    }
}

fn invoke(setup: &Setup, role: Role, line: &str) {
    setup.handles.invoke(
        Actor::Player("someone".into()),
        role,
        Lang::En,
        line,
        ReplyTo::Nowhere,
    );
}

/// Waits for a vote to be published as running.
async fn vote_started(events: &mut broadcast::Receiver<Event>) -> bool {
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let Ok(Event::Custom { name, payload, .. }) = events.recv().await
                && name == "state"
                && payload["active"] == true
            {
                return;
            }
        }
    })
    .await
    .is_ok()
}

async fn called(server: &MockServer, method: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !server.method_names().iter().any(|m| m == method) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn admins_skip_without_a_vote() {
    let mut setup = setup().await;
    invoke(&setup, Role::Admin, "/skip");
    assert!(called(&setup.server, "NextMap").await);
    assert!(!vote_started(&mut setup.events).await);
    setup.shutdown.cancel();
}

#[tokio::test]
async fn referees_restart_without_a_vote() {
    let mut setup = setup().await;
    invoke(&setup, Role::Referee, "/res");
    assert!(called(&setup.server, "RestartMap").await);
    assert!(!vote_started(&mut setup.events).await);
    setup.shutdown.cancel();
}

#[tokio::test]
async fn players_and_referees_call_a_skip_vote() {
    for role in [Role::Player, Role::Referee] {
        let mut setup = setup().await;
        invoke(&setup, role, "/skip");
        assert!(vote_started(&mut setup.events).await, "{role:?}");
        setup.shutdown.cancel();
    }
}
