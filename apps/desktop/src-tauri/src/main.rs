//! The Boxquell desktop app: one window that shows the daemon's web UI.
//!
//! The app holds no logic of its own. It finds the running daemon or starts
//! one, then opens the same page `boxquell ui` would open in a browser.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{bail, Context, Result};
use boxquell_core::Home;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::{WebviewUrl, WebviewWindowBuilder};

/// What the daemon writes when it is up.
#[derive(serde::Deserialize)]
struct DaemonFile {
    port: u16,
    token: String,
}

/// The address of a daemon that answers, with its token in the fragment.
fn running(home: &Home) -> Option<String> {
    let file: DaemonFile = serde_json::from_slice(&std::fs::read(home.daemon_file()).ok()?).ok()?;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], file.port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
    Some(format!("http://127.0.0.1:{}/#token={}", file.port, file.token))
}

/// The `boxquell` program: named by `BOXQUELL_BIN`, next to this app, or on the PATH.
fn boxquell_binary() -> PathBuf {
    let name = if cfg!(windows) { "boxquell.exe" } else { "boxquell" };
    if let Some(path) = std::env::var_os("BOXQUELL_BIN") {
        return PathBuf::from(path);
    }
    let beside = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(name)));
    beside.filter(|path| path.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

/// Find the daemon or start it, and wait until it answers.
fn daemon_url() -> Result<String> {
    let home = Home::locate()?;
    if let Some(url) = running(&home) {
        return Ok(url);
    }
    let binary = boxquell_binary();
    let log = std::fs::File::create(home.root().join("daemon.log"))?;
    let mut command = Command::new(&binary);
    command.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    // The daemon outlives this window: sandboxes keep running when the app is closed.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().with_context(|| format!("The boxquell command was not found ({}).", binary.display()))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(url) = running(&home) {
            return Ok(url);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    bail!("The background service did not come up; see {}.", home.root().join("daemon.log").display())
}

/// Port and token of the running daemon, for the tray menu's own requests.
fn daemon_access() -> Option<DaemonFile> {
    serde_json::from_slice(&std::fs::read(Home::locate().ok()?.daemon_file()).ok()?).ok()
}

/// Ask the daemon whether the emergency stop is on, or switch it.
fn lockdown(set: Option<bool>) -> Option<bool> {
    let access = daemon_access()?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(5))).build().into();
    let url = format!("http://127.0.0.1:{}/api/lockdown", access.port);
    let bearer = format!("Bearer {}", access.token);
    let mut response = match set {
        Some(on) => agent.post(&url).header("Authorization", &bearer).send_json(serde_json::json!({ "on": on })).ok()?,
        None => agent.get(&url).header("Authorization", &bearer).call().ok()?,
    };
    response.body_mut().read_json::<serde_json::Value>().ok()?["on"].as_bool()
}

fn show_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    match app.get_webview_window("main") {
        Some(window) => {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        None => {
            let _ = open_window(app);
        }
    }
}

fn open_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let page = match daemon_url() {
        Ok(url) => WebviewUrl::External(url.parse().expect("the daemon address is a URL")),
        // The bundled page explains what went wrong.
        Err(err) => WebviewUrl::App(format!("index.html#{err:#}").into()),
    };
    // The daemon shows its interface to this app; browsers have to be allowed in the settings.
    WebviewWindowBuilder::new(app, "main", page).user_agent("Boxquell-Desktop").title("Boxquell").inner_size(1360.0, 900.0).min_inner_size(760.0, 520.0).build()?;
    Ok(())
}

const LOCKDOWN_ON: &str = "Emergency stop: block all network";
const LOCKDOWN_OFF: &str = "Lift the network lockdown";

fn main() {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;

    tauri::Builder::default()
        .setup(|app| {
            open_window(app.handle())?;

            // A tray icon keeps Boxquell at hand while its window is closed.
            let open = MenuItem::with_id(app, "open", "Open Boxquell", true, None::<&str>)?;
            let locked = lockdown(None).unwrap_or(false);
            let stop = MenuItem::with_id(app, "lockdown", if locked { LOCKDOWN_OFF } else { LOCKDOWN_ON }, true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit (sandboxes keep running)", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &stop, &PredefinedMenuItem::separator(app)?, &quit])?;
            let stop_item = stop.clone();
            let mut tray = TrayIconBuilder::with_id("boxquell").tooltip("Boxquell").menu(&menu).show_menu_on_left_click(true).on_menu_event(move |app, event| match event.id.as_ref() {
                "open" => show_window(app),
                "lockdown" => {
                    // Read the state fresh: it may have been changed elsewhere.
                    let now = lockdown(None).unwrap_or(false);
                    if let Some(on) = lockdown(Some(!now)) {
                        let _ = stop_item.set_text(if on { LOCKDOWN_OFF } else { LOCKDOWN_ON });
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        // Closing the window leaves the app in the tray; Quit in the tray menu ends it.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("the window could not be created");
}
