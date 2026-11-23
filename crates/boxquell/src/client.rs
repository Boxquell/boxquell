//! CLI side of the daemon API. Starts the daemon on first use.

use crate::daemon::DaemonFile;
use anyhow::{anyhow, bail, Context, Result};
use boxquell_core::Home;
use serde_json::Value;
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Client {
    base: String,
    token: String,
}

impl Client {
    fn from_file(home: &Home) -> Option<Self> {
        let file: DaemonFile = serde_json::from_slice(&std::fs::read(home.daemon_file()).ok()?).ok()?;
        let client = Self { base: format!("http://127.0.0.1:{}", file.port), token: file.token };
        client.get("/api/ping").is_ok().then_some(client)
    }

    /// Address of the web UI; the token travels in the fragment, which browsers never send to a server.
    pub fn ui_url(&self) -> String {
        format!("{}/#token={}", self.base, self.token)
    }

    /// Connect to the running daemon, if there is one.
    pub fn existing(home: &Home) -> Option<Self> {
        Self::from_file(home)
    }

    /// Connect to the daemon, launching it in the background when needed.
    pub fn connect(home: &Home) -> Result<Self> {
        if let Some(client) = Self::from_file(home) {
            return Ok(client);
        }
        let log = std::fs::File::create(home.root().join("daemon.log"))?;
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
        #[cfg(unix)]
        {
            // Own process group, so Ctrl-C in the terminal does not stop the sandboxes.
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        }
        command.spawn().context("cannot start the boxquell daemon")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(client) = Self::from_file(home) {
                return Ok(client);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("the boxquell daemon did not come up; see {}", home.root().join("daemon.log").display())
    }

    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        // Starting a sandbox for the first time copies its disk image, which can take a while.
        let request = ureq::request(method, &format!("{}{path}", self.base))
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(300));
        let response = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        };
        match response {
            Ok(response) => Ok(response.into_json()?),
            Err(ureq::Error::Status(_, response)) => {
                let body: Value = response.into_json().unwrap_or_default();
                Err(anyhow!("{}", body["error"].as_str().unwrap_or("request failed")))
            }
            Err(err) => Err(anyhow!("cannot reach the boxquell daemon: {err}")),
        }
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.request("GET", path, None)
    }
    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request("POST", path, Some(body))
    }
    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.request("PATCH", path, Some(body))
    }
    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.request("PUT", path, Some(body))
    }
    pub fn delete(&self, path: &str, body: Option<Value>) -> Result<Value> {
        self.request("DELETE", path, body)
    }

    /// Call `on_event` for every new log entry until the daemon goes away.
    pub fn follow(&self, query: &str, mut on_event: impl FnMut(&Value)) -> Result<()> {
        let response = ureq::get(&format!("{}/api/events/stream?{query}", self.base))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|err| anyhow!("cannot reach the boxquell daemon: {err}"))?;
        for line in std::io::BufReader::new(response.into_reader()).lines() {
            if let Some(event) = line?.strip_prefix("data:").and_then(|data| serde_json::from_str(data.trim()).ok()) {
                on_event(&event);
            }
        }
        Ok(())
    }
}
