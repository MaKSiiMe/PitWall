//! Server console: commands typed on stdin run as SuperAdmin.

use pitwall_core::tmtext::strip_formatting;
use pitwall_core::{Actor, Handles, ReplyTo, Role};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Reads stdin line by line until EOF or shutdown. `quit` stops the controller.
pub async fn run(handles: Handles, shutdown: CancellationToken) {
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<String>();
    let printer = tokio::spawn(async move {
        while let Some(line) = reply_rx.recv().await {
            // The console shows plain text: drop the in-game color codes.
            println!("{}", strip_formatting(&line));
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = tokio::select! {
            line = lines.next_line() => line,
            () = shutdown.cancelled() => break,
        };
        match line {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if matches!(line, "quit" | "exit" | "stop") {
                    tracing::info!("stop requested from the console");
                    shutdown.cancel();
                    break;
                }
                handles.invoke(
                    Actor::Console,
                    Role::SuperAdmin,
                    handles.default_lang,
                    line,
                    ReplyTo::Channel(reply_tx.clone()),
                );
            }
            Ok(None) => {
                tracing::debug!("stdin closed, console disabled");
                break;
            }
            Err(err) => {
                tracing::warn!(%err, "cannot read stdin, console disabled");
                break;
            }
        }
    }
    drop(reply_tx);
    let _ = printer.await;
}
