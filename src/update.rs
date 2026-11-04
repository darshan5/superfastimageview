//! Update checker: asks GitHub for the latest published release (drafts and
//! pre-releases don't count) and compares its version with this build.
//!
//! The request runs `curl` (built into macOS and Windows 10+) on a
//! background thread, so the app needs no HTTP or TLS library and never waits
//! for the network. Automatic checks happen once a month (also while the app
//! stays open), and only after the first image is on screen.

use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};

/// The GitHub repository releases are published to.
pub const REPO: &str = "darshan5/superfastimageview";

/// How often the app checks for a new release on its own.
const CHECK_INTERVAL_SECS: i64 = 30 * 24 * 60 * 60;

/// How long to wait before retrying a failed automatic check.
const RETRY_SECS: i64 = 24 * 60 * 60;

/// The newest release's version (without the leading `v`), or an error.
pub type CheckResult = Result<String, String>;

/// This build's version.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The numeric parts of `1.2.3` (a `v` prefix and `-beta` style suffixes are
/// ignored).
fn parts(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Is version `latest` newer than `current`?
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The `"tag_name": "..."` value from GitHub's release JSON.
fn tag_name(json: &str) -> Option<&str> {
    let rest = &json[json.find("\"tag_name\"")? + "\"tag_name\"".len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// Reads GitHub's "latest release" JSON into the release's version.
fn parse_latest(json: &str) -> CheckResult {
    match tag_name(json) {
        Some(tag) => Ok(tag.trim_start_matches('v').to_string()),
        // e.g. {"message": "Not Found"} before the first release is published.
        None if json.contains("\"message\"") => Err("GitHub has no release yet".into()),
        None => Err("Unexpected reply from GitHub".into()),
    }
}

/// The web page for a release.
pub fn release_page(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{version}")
}

/// Opens a web page in the default browser.
pub fn open_page(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Asks GitHub for the latest release (blocking; call it off the UI thread).
fn check() -> CheckResult {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        "15",
        "--header",
        "Accept: application/vnd.github+json",
        "--user-agent",
        concat!("superfastimageview/", env!("CARGO_PKG_VERSION")),
        &url,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("Couldn't run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("curl: ").to_string();
        return Err(if err.is_empty() { "Couldn't reach GitHub".into() } else { format!("Couldn't reach GitHub: {err}") });
    }
    parse_latest(&String::from_utf8_lossy(&out.stdout))
}

/// Seconds since 1970, for remembering when the last check ran.
pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Is an automatic check due, given when the last one succeeded? A clock
/// that went back past it counts as due.
fn check_due(last_check: i64, retry_after: i64, now: i64) -> bool {
    now >= retry_after && (now - last_check >= CHECK_INTERVAL_SECS || now < last_check)
}

/// Update checking as the app sees it.
#[derive(Default)]
pub struct UpdateState {
    /// No automatic check before this time (seconds since 1970); set after
    /// a failed one so it isn't retried every frame.
    pub retry_after: i64,
    pub job: Option<Receiver<CheckResult>>,
    /// The running check was started from Settings (so report the result).
    pub manual: bool,
    /// The result of the last check started from Settings, for display.
    pub status: Option<String>,
}

impl UpdateState {
    /// Starts a check on a background thread, waking the UI when it's done.
    pub fn start(&mut self, ctx: &egui::Context, manual: bool) {
        self.manual |= manual;
        if manual {
            self.status = Some("Checking…".into());
        }
        if self.job.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(check());
            ctx.request_repaint();
        });
        self.job = Some(rx);
    }

    /// Starts an automatic check if one is due and none is running.
    pub fn start_if_due(&mut self, ctx: &egui::Context, last_check: i64) {
        if self.job.is_none() && check_due(last_check, self.retry_after, now_secs()) {
            self.start(ctx, false);
        }
    }

    /// The result of a finished check, if one just finished.
    pub fn poll(&mut self) -> Option<CheckResult> {
        let result = self.job.as_ref()?.try_recv().ok()?;
        self.job = None;
        if result.is_err() {
            self.retry_after = now_secs() + RETRY_SECS;
        }
        let manual = std::mem::take(&mut self.manual);
        if manual {
            self.status = Some(match &result {
                Ok(v) if is_newer(v, current_version()) => format!("Version {v} is available."),
                Ok(_) => format!("You have the latest version ({}).", current_version()),
                Err(e) => e.clone(),
            });
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("1.0.0-beta", "1.0.0"));
    }

    #[test]
    fn reads_github_replies() {
        assert_eq!(parse_latest(r#"{"url": "x", "tag_name": "v0.3.1", "name": "y"}"#), Ok("0.3.1".into()));
        assert_eq!(parse_latest(r#"{"tag_name" : "0.4.0"}"#), Ok("0.4.0".into()));
        assert!(parse_latest(r#"{"message": "Not Found", "status": "404"}"#).is_err());
        assert!(parse_latest("<html>").is_err());
    }

    #[test]
    fn checks_once_a_month() {
        let day = 24 * 60 * 60;
        let now = 1_800_000_000;
        assert!(check_due(0, 0, now), "never checked");
        assert!(!check_due(now - 29 * day, 0, now));
        assert!(check_due(now - 30 * day, 0, now));
        assert!(!check_due(now - 40 * day, now + 1, now), "retry waits");
        assert!(check_due(now + day, 0, now), "clock went back");
    }

    /// Asks the real GitHub (network): `cargo test --release live_check -- --ignored --nocapture`
    #[test]
    #[ignore = "network"]
    fn live_check() {
        println!("{:?}", check());
    }
}
