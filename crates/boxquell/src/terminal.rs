//! Terminals for the web UI: a WebSocket whose other end is `ssh` into the
//! sandbox, running in a pseudo-terminal on the host.
//!
//! Binary frames carry terminal bytes in both directions. A text frame from
//! the browser is a control message: `{"resize":[cols,rows]}`.

use crate::daemon::Daemon;
use crate::ssh;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use boxquell_core::drive::shell_quote;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct TerminalQuery {
    /// Command to run in /workspace, e.g. an agent; a login shell when absent.
    cmd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct Control {
    resize: Option<(u16, u16)>,
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { cols: cols.clamp(2, 500), rows: rows.clamp(2, 300), pixel_width: 0, pixel_height: 0 }
}

pub async fn open(
    State(daemon): State<Arc<Daemon>>,
    Path(name): Path<String>,
    Query(query): Query<TerminalQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Starting here turns a sandbox that cannot boot into a readable HTTP error.
    let info = match daemon.start(&name).await {
        Ok(info) => info,
        Err(err) => return (StatusCode::BAD_REQUEST, format!("{err:#}")).into_response(),
    };
    let (Some(key), Some(port)) = (info["ssh_key"].as_str().map(PathBuf::from), info["ssh_port"].as_u64()) else {
        return (StatusCode::BAD_REQUEST, "the sandbox has no ssh access").into_response();
    };
    upgrade.on_upgrade(move |socket| async move {
        if let Err(err) = session(socket, key, port as u16, query).await {
            eprintln!("terminal for {name}: {err:#}");
        }
    })
}

async fn session(mut socket: WebSocket, key: PathBuf, port: u16, query: TerminalQuery) -> anyhow::Result<()> {
    let pair = native_pty_system().openpty(size(query.cols.unwrap_or(80), query.rows.unwrap_or(24)))?;
    let mut command = CommandBuilder::new("ssh");
    command.arg("-tt");
    command.args(ssh::base_args(&key, port));
    if let Some(cmd) = query.cmd.as_deref().map(str::trim).filter(|cmd| !cmd.is_empty()) {
        // A login shell loads the sandbox environment (placeholders, CA paths) and goes to the project root.
        command.arg(format!("bash -lc {}", shell_quote(&format!("exec {cmd}"))));
    }
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            if tx.blocking_send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            output = rx.recv() => match output {
                Some(bytes) => {
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                // The command ended.
                None => break,
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Binary(bytes))) => writer.write_all(&bytes)?,
                Some(Ok(Message::Text(text))) => {
                    if let Ok(Control { resize: Some((cols, rows)) }) = serde_json::from_str(text.as_str()) {
                        pair.master.resize(size(cols, rows))?;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = socket.send(Message::Close(None)).await;
    Ok(())
}
