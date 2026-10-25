//! Replays a recording as a fake server, to reproduce a session against a real PitWall:
//! `cargo run -p pitwall-gbx --features mock --example replay -- <trace.jsonl> [speed]`,
//! then `PITWALL_SERVER__PASSWORD=dev cargo run` in another terminal. The server listens on
//! 127.0.0.1:5000 and starts playing the callbacks once PitWall is connected; `speed` divides
//! the recorded gaps (1 = real time, the default).

use std::process::ExitCode;
use std::time::Duration;

use pitwall_gbx::mock::MockServer;
use pitwall_gbx::replay::Trace;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: replay <trace.jsonl> [speed]");
        return ExitCode::FAILURE;
    };
    let speed: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let trace = match Trace::load(std::path::Path::new(path)) {
        Ok(trace) => trace,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let server = match MockServer::with_handler_on("127.0.0.1:5000", trace.handler()).await {
        Ok(server) => server,
        Err(err) => {
            eprintln!("cannot listen on 127.0.0.1:5000: {err}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "replaying {} callbacks on {}, waiting for PitWall…",
        trace.callback_count(),
        server.addr()
    );
    while !server.method_names().iter().any(|m| m == "EnableCallbacks") {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    trace.play(&server, speed, Duration::from_secs(60)).await;
    println!("all callbacks sent; Ctrl+C to stop");
    let _ = tokio::signal::ctrl_c().await;
    ExitCode::SUCCESS
}
