//! Recording and replay of the GbxRemote traffic (docs/ENREGISTREMENT.fr.md).
//!
//! `replay(trace)` plays a recorded session against a fresh controller and returns the events it
//! published: a bug seen on the server becomes a test by recording it, anonymizing the file into
//! `tests/traces/` and asserting on those events.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use pitwall_core::{
    Controller, ControllerOptions, Event, Lang, RoleBook, RolesConfig, ServerConfig, Translator,
};
use pitwall_gbx::mock::MockServer;
use pitwall_gbx::record::{Recorder, RecorderConfig, recordings};
use pitwall_gbx::replay::{Trace, anonymize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

struct Running {
    events: broadcast::Receiver<Event>,
    shutdown: CancellationToken,
}

async fn start(server: &MockServer, recorder: Option<Recorder>) -> Running {
    let (host, port) = server
        .addr()
        .rsplit_once(':')
        .map(|(h, p)| (h.to_owned(), p.parse().unwrap()))
        .unwrap();
    let controller = Controller::new(
        ControllerOptions {
            server: ServerConfig {
                host,
                port,
                password: "xmlrpc-secret".to_owned(),
                ..ServerConfig::default()
            },
            default_lang: Lang::En,
            recorder,
            ..ControllerOptions::default()
        },
        RoleBook::from_config(&RolesConfig::default()),
        Translator::with_core_catalogs().unwrap(),
    );
    let mut events = controller.handles().bus.raw_subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(controller.run(shutdown.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(events.recv().await, Ok(Event::Connected { .. })) {}
    })
    .await
    .unwrap();
    Running { events, shutdown }
}

/// Plays `trace` against a fresh controller; returns the events published meanwhile.
async fn replay(trace: &Trace) -> Vec<Event> {
    let server = MockServer::with_handler(trace.handler()).await.unwrap();
    let mut running = start(&server, None).await;
    trace.play(&server, 100.0, Duration::from_millis(50)).await;
    // Let the controller handle the last callbacks.
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.shutdown.cancel();
    let mut events = Vec::new();
    while let Ok(event) = running.events.try_recv() {
        events.push(event);
    }
    events
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pitwall-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A real login format, so that the anonymization has something to replace.
const BOB: &str = "L9H2ZIXkRCmJytSeTSCDoQ";

/// A short Time Attack session: bob joins, chats, finishes, types a password.
async fn play_session(server: &MockServer) {
    server.send_callback("ManiaPlanet.PlayerConnect", vec![BOB.into(), false.into()]);
    server.send_callback(
        "ManiaPlanet.PlayerChat",
        vec![2.into(), BOB.into(), "salut".into(), false.into(), 0.into()],
    );
    server.send_script_callback(
        "Trackmania.Event.WayPoint",
        &serde_json::json!({"login": BOB, "racetime": 33_333, "isendrace": true,
            "curracecheckpoints": [11_111, 33_333]}),
    );
    server.send_callback(
        "ManiaPlanet.PlayerChat",
        vec![
            2.into(),
            BOB.into(),
            "/password chat-secret".into(),
            false.into(),
            0.into(),
        ],
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
}

fn finished(events: &[Event], login: &str, time: i32) -> bool {
    events.iter().any(|e| {
        matches!(e, Event::Finish { waypoint } if waypoint.login == login && waypoint.racetime == time)
    })
}

fn joined(events: &[Event], login: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::PlayerJoined { player } if player.login == login))
}

#[tokio::test]
async fn a_recorded_session_replays_with_its_secrets_masked() {
    let dir = temp_dir("record");
    let recorder = Recorder::start(RecorderConfig {
        dir: dir.clone(),
        max_file_bytes: 10 * 1024 * 1024,
        max_files: 3,
    })
    .unwrap();
    let server = MockServer::start().await.unwrap();
    let running = start(&server, Some(recorder)).await;
    play_session(&server).await;
    running.shutdown.cancel();
    // The writer flushes when idle.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let files = recordings(&dir).unwrap();
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("Authenticate") && text.contains("\"dir\":\"out\""));
    assert!(text.contains("\"event\":\"connected\""));
    assert!(!text.contains("xmlrpc-secret"), "XML-RPC password recorded");
    assert!(
        !text.contains("chat-secret"),
        "password typed in the chat recorded"
    );

    let events = replay(&Trace::parse(&text).unwrap()).await;
    assert!(joined(&events, BOB), "{events:?}");
    assert!(finished(&events, BOB, 33_333), "{events:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn recordings_rotate_and_keep_the_newest_files() {
    let dir = temp_dir("rotate");
    let recorder = Recorder::start(RecorderConfig {
        dir: dir.clone(),
        max_file_bytes: 1,
        max_files: 2,
    })
    .unwrap();
    for i in 0..5 {
        recorder.received(i, b"<?xml version=\"1.0\"?><methodCall><methodName>x</methodName><params></params></methodCall>");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(recordings(&dir).unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sample trace of the repository, anonymized from a recording.
fn sample() -> Trace {
    Trace::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces/ta-session.jsonl"))
        .unwrap()
}

#[tokio::test]
async fn the_sample_trace_replays() {
    let trace = sample();
    assert!(trace.callback_count() >= 3);
    let events = replay(&trace).await;
    // Logins are anonymized: the player is the first fake login.
    let login = events
        .iter()
        .find_map(|e| match e {
            Event::PlayerJoined { player } => Some(player.login.clone()),
            _ => None,
        })
        .expect("a player joins");
    assert!(finished(&events, &login, 33_333), "{events:?}");
}

#[test]
fn the_sample_trace_is_anonymized() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces/ta-session.jsonl"),
    )
    .unwrap();
    assert_eq!(
        anonymize(&text).unwrap(),
        text,
        "run the anonymize example on it"
    );
    assert!(!text.contains("salut") && !text.contains(BOB));
}
