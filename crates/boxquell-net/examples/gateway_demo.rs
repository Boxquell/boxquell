//! Run a gateway for one VM and print its events as JSON lines.
//! usage: gateway_demo <socket> ['allow host[:port]' | 'deny host[:port]']...
use boxquell_net::{Policy, SystemUpstream};
use std::sync::{Arc, RwLock};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: gateway_demo <socket> [rule]...");
    let rules = args.map(|r| r.parse().map_err(anyhow::Error::msg)).collect::<anyhow::Result<Vec<_>>>()?;
    let _ = std::fs::remove_file(&path);
    let (link, _) = tokio::net::UnixListener::bind(&path)?.accept().await?;
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let gateway = boxquell_net::spawn(link, SystemUpstream, Arc::new(RwLock::new(Policy { rules, ..Default::default() })), None, events_tx);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            println!("{}", serde_json::to_string(&event).unwrap());
        }
    });
    gateway.closed().await;
    Ok(())
}
