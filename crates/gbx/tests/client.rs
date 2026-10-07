//! End-to-end tests of the client against the in-process mock server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use pitwall_gbx::mock::MockServer;
use pitwall_gbx::xml::MethodCall;
use pitwall_gbx::{Callback, ClientOptions, Fault, GbxClient, GbxError, ScriptCallback, Value};

async fn connect(server: &MockServer) -> (GbxClient, pitwall_gbx::Callbacks) {
    GbxClient::connect(&server.addr(), ClientOptions::default())
        .await
        .expect("connect to mock server")
}

#[tokio::test]
async fn authenticates_and_reads_version() {
    let server = MockServer::start().await.unwrap();
    let (client, _callbacks) = connect(&server).await;

    client.authenticate("SuperAdmin", "secret").await.unwrap();
    client.set_api_version("2023-04-24").await.unwrap();
    let version = client.get_version().await.unwrap();
    assert_eq!(version.name, "Trackmania");

    let calls = server.calls();
    assert_eq!(calls[0].method, "Authenticate");
    assert_eq!(
        calls[0].params,
        vec![Value::from("SuperAdmin"), Value::from("secret")]
    );
}

#[tokio::test]
async fn faults_are_typed_errors() {
    let server = MockServer::start().await.unwrap();
    let (client, _callbacks) = connect(&server).await;
    let err = client
        .authenticate("SuperAdmin", "wrong")
        .await
        .unwrap_err();
    assert!(
        matches!(err, GbxError::Fault(Fault { code: -1000, .. })),
        "{err:?}"
    );
}

#[tokio::test]
async fn concurrent_calls_get_their_own_responses() {
    let server = MockServer::with_handler(|call: &MethodCall| Ok(call.params[0].clone()))
        .await
        .unwrap();
    let (client, _callbacks) = connect(&server).await;
    let mut tasks = Vec::new();
    for i in 0..50 {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            (i, client.call("Echo", vec![i.into()]).await)
        }));
    }
    for task in tasks {
        let (i, result) = task.await.unwrap();
        assert_eq!(result.unwrap(), Value::Int(i));
    }
}

#[tokio::test]
async fn receives_typed_callbacks() {
    let server = MockServer::start().await.unwrap();
    let (client, mut callbacks) = connect(&server).await;
    // Make sure the connection is fully set up on the server side before pushing.
    client.enable_callbacks(true).await.unwrap();

    server.send_callback(
        "ManiaPlanet.PlayerConnect",
        vec!["abc".into(), false.into()],
    );
    server.send_script_callback(
        "Trackmania.Event.WayPoint",
        &serde_json::json!({"login": "abc", "racetime": 4242, "isendrace": true, "curracecheckpoints": [1000, 4242]}),
    );

    let first = tokio::time::timeout(Duration::from_secs(2), callbacks.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first,
        Callback::PlayerConnect {
            login: "abc".into(),
            is_spectator: false
        }
    );

    let second = tokio::time::timeout(Duration::from_secs(2), callbacks.recv())
        .await
        .unwrap()
        .unwrap();
    let Callback::Script(ScriptCallback::WayPoint(wp)) = second else {
        panic!("unexpected {second:?}");
    };
    assert_eq!(
        (wp.login.as_str(), wp.racetime, wp.isendrace),
        ("abc", 4242, true)
    );
}

#[tokio::test]
async fn multicall_mixes_results_and_faults() {
    let server = MockServer::with_handler(|_call: &MethodCall| {
        Ok(Value::Array(vec![
            Value::Array(vec![Value::Bool(true)]),
            Value::Struct(
                [
                    ("faultCode".to_owned(), Value::Int(-1000)),
                    ("faultString".to_owned(), Value::from("Not in script mode.")),
                ]
                .into_iter()
                .collect(),
            ),
        ]))
    })
    .await
    .unwrap();
    let (client, _callbacks) = connect(&server).await;
    let results = client
        .multicall(vec![
            ("RestartMap".into(), vec![]),
            ("SetScriptName".into(), vec!["x".into()]),
        ])
        .await
        .unwrap();
    assert_eq!(results[0], Ok(Value::Bool(true)));
    assert_eq!(results[1].as_ref().unwrap_err().code, -1000);
}

#[tokio::test]
async fn timeout_when_server_never_answers() {
    // A raw listener that does the handshake and then stays silent.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (mut stream, _) = listener.accept().await.unwrap();
        stream
            .write_all(b"\x0b\x00\x00\x00GBXRemote 2")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let options = ClientOptions {
        call_timeout: Duration::from_millis(100),
        ..ClientOptions::default()
    };
    let (client, _callbacks) = GbxClient::connect(&addr, options).await.unwrap();
    let err = client.restart_map().await.unwrap_err();
    assert!(matches!(err, GbxError::Timeout { .. }), "{err:?}");
}

#[tokio::test]
async fn pending_calls_fail_when_the_connection_drops() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = listener.accept().await.unwrap();
        stream
            .write_all(b"\x0b\x00\x00\x00GBXRemote 2")
            .await
            .unwrap();
        let mut buf = [0u8; 8];
        let _ = stream.read(&mut buf).await;
        // Close without answering.
    });
    let (client, mut callbacks) = GbxClient::connect(&addr, ClientOptions::default())
        .await
        .unwrap();
    let err = client.restart_map().await.unwrap_err();
    assert!(matches!(err, GbxError::ConnectionClosed), "{err:?}");
    assert!(
        callbacks.recv().await.is_none(),
        "callback stream ends with the connection"
    );
    assert!(client.is_closed());
}

#[tokio::test]
async fn rejects_servers_that_are_not_gbxremote2() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (mut stream, _) = listener.accept().await.unwrap();
        stream
            .write_all(b"\x0b\x00\x00\x00GBXRemote 1")
            .await
            .unwrap();
    });
    let err = GbxClient::connect(&addr, ClientOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, GbxError::Handshake(_)), "{err:?}");
}
