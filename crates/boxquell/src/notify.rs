//! Desktop notifications through what each system brings along, so no
//! extra library is needed. Failing to show one is never an error.

use std::process::{Command, Stdio};

pub fn show(title: &str, body: &str) {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("osascript");
        // The texts travel as arguments, never as script source.
        command.args(["-e", "on run argv", "-e", "display notification (item 2 of argv) with title (item 1 of argv)", "-e", "end run", title, body]);
        command
    } else if cfg!(windows) {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_TOAST]).env("BOXQUELL_TITLE", title).env("BOXQUELL_BODY", body);
        command
    } else {
        let mut command = Command::new("notify-send");
        command.args(["--app-name", "Boxquell", "--icon", "network-error", "--", title, body]);
        command
    };
    // Reap the child in the background so it does not linger as a zombie.
    if let Ok(mut child) = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Shows a toast with the texts from the environment.
const WINDOWS_TOAST: &str = "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
$xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
$text = $xml.GetElementsByTagName('text'); \
$text.Item(0).AppendChild($xml.CreateTextNode($env:BOXQUELL_TITLE)) > $null; \
$text.Item(1).AppendChild($xml.CreateTextNode($env:BOXQUELL_BODY)) > $null; \
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Boxquell').Show([Windows.UI.Notifications.ToastNotification]::new($xml))";
