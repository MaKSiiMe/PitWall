//! A fake dedicated server on 127.0.0.1:5000 to try PitWall without Trackmania.
//!
//! ```sh
//! cargo run -p pitwall-gbx --features mock --example mock_server
//! ```
//!
//! It accepts any password except `wrong`, tracks the mode script like a real server
//! (`LoadMatchSettings` + `RestartMap`), simulates one player racing and announces map loads.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pitwall_gbx::Value;
use pitwall_gbx::mock::{MockServer, default_handler};
use pitwall_gbx::xml::MethodCall;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("MOCK_BIND").unwrap_or_else(|_| "127.0.0.1:5000".to_owned());
    // (current script, next script, last written file, restart requested)
    let state = Arc::new(Mutex::new((
        "Trackmania/TM_TimeAttack_Online.Script.txt".to_owned(),
        "Trackmania/TM_TimeAttack_Online.Script.txt".to_owned(),
        String::new(),
        false,
    )));
    let s = Arc::clone(&state);
    let server = MockServer::with_handler_on(&bind, move |call: &MethodCall| {
        let mut st = s.lock().unwrap_or_else(PoisonError::into_inner);
        println!("← {}({} params)", call.method, call.params.len());
        let pair = |a: &str, b: Value| (a.to_owned(), b);
        match call.method.as_str() {
            "WriteFile" => {
                if let Some(Value::Base64(data)) = call.params.get(1) {
                    st.2 = String::from_utf8_lossy(data).into_owned();
                    println!("   file content:\n{}", st.2);
                }
                Ok(true.into())
            }
            "LoadMatchSettings" => {
                let file = st.2.clone();
                if let Some(start) = file.find("<script_name>") {
                    let rest = &file[start + 13..];
                    if let Some(end) = rest.find("</script_name>") {
                        st.1 = format!("{}.Script.txt", &rest[..end]);
                    }
                }
                Ok(1.into())
            }
            "RestartMap" | "NextMap" => {
                st.0 = st.1.clone();
                st.3 = true;
                Ok(true.into())
            }
            "GetScriptName" => Ok(Value::Struct(
                [
                    pair("CurrentValue", st.0.clone().into()),
                    pair("NextValue", st.1.clone().into()),
                ]
                .into_iter()
                .collect(),
            )),
            "GetMapList" => Ok(Value::Array(vec![Value::Struct(
                [
                    pair("UId", "mockuid".into()),
                    pair("FileName", "Campaigns/Mock.Map.Gbx".into()),
                ]
                .into_iter()
                .collect(),
            )])),
            "GetCurrentMapIndex" => Ok(0.into()),
            "GetPlayerList" => Ok(Value::Array(vec![Value::Struct(
                [
                    pair("Login", "mock_player".into()),
                    pair("NickName", "$0afMock $fffPlayer".into()),
                    pair("PlayerId", 1.into()),
                    pair("TeamId", (-1).into()),
                    pair("SpectatorStatus", 0.into()),
                ]
                .into_iter()
                .collect(),
            )])),
            "GetDetailedPlayerInfo" => Ok(Value::Struct(
                [
                    pair("Login", call.params[0].clone()),
                    pair("NickName", "$0afMock $fffPlayer".into()),
                    pair("Language", "fr".into()),
                    pair("Path", "World|Europe|France".into()),
                ]
                .into_iter()
                .collect(),
            )),
            _ => default_handler(call),
        }
    })
    .await?;
    println!("Mock GbxRemote server listening on {}", server.addr());

    // Simulated race: a checkpoint every 3 s, a finish every 4 checkpoints, map reloads after restarts.
    let mut cp = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let restarted = std::mem::take(&mut state.lock().unwrap_or_else(PoisonError::into_inner).3);
        if restarted {
            server.send_script_callback(
                "Maniaplanet.StartMap_Start",
                &serde_json::json!({"count": 1, "restarted": true, "time": 0, "map": {"uid": "mockuid", "name": "Mock"}}),
            );
            cp = 0;
            continue;
        }
        cp += 1;
        let times: Vec<i32> = (1..=cp).map(|i| i * 7_250).collect();
        server.send_script_callback(
            "Trackmania.Event.WayPoint",
            &serde_json::json!({
                "login": "mock_player", "accountid": "00000000-0000-0000-0000-000000000001",
                "racetime": cp * 7_250, "checkpointinrace": cp - 1, "isendrace": cp == 4,
                "curracecheckpoints": times, "speed": 420.0
            }),
        );
        if cp == 4 {
            cp = 0;
        }
    }
}
