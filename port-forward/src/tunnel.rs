use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex as TokioMutex};
use which::which;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    TryCloudflare,
    LocalhostRun,
    Localtunnel,
}

impl Provider {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "trycloudflare" => Some(Self::TryCloudflare),
            "localhost.run" => Some(Self::LocalhostRun),
            "localtunnel" => Some(Self::Localtunnel),
            _ => None,
        }
    }

    pub fn binary_name(self) -> &'static str {
        match self {
            Self::TryCloudflare => "cloudflared",
            Self::LocalhostRun => "ssh",
            Self::Localtunnel => "lt",
        }
    }

    pub fn install_hint(self) -> Option<&'static str> {
        match self {
            Self::TryCloudflare => {
                Some("Download cloudflared from https://github.com/cloudflare/cloudflared/releases")
            }
            Self::LocalhostRun => {
                Some("Install OpenSSH client via your system package manager (apt, brew, etc.)")
            }
            Self::Localtunnel => Some("Run: npm install -g localtunnel  (requires Node.js/npm)"),
        }
    }

    pub fn args(self, port: u16) -> Vec<String> {
        match self {
            Self::TryCloudflare => {
                vec![
                    "tunnel".into(),
                    "--url".into(),
                    format!("http://127.0.0.1:{}", port),
                    "--no-autoupdate".into(),
                ]
            }
            Self::LocalhostRun => {
                vec![
                    "-o".into(),
                    "StrictHostKeyChecking=no".into(),
                    "-R".into(),
                    format!("*:80:127.0.0.1:{}", port),
                    "nokey@localhost.run".into(),
                ]
            }
            Self::Localtunnel => {
                vec![format!("--port={}", port)]
            }
        }
    }

    pub fn parse_url(self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        match self {
            Self::TryCloudflare => trimmed
                .split_whitespace()
                .find(|w| w.starts_with("https://") && w.contains("trycloudflare.com"))
                .map(|w| w.trim_end_matches(',').to_string()),
            Self::LocalhostRun => trimmed
                .split_whitespace()
                .find(|w| {
                    (w.starts_with("https://") || w.starts_with("http://"))
                        && w.contains("lhr.life")
                })
                .map(|w| w.trim_end_matches(',').to_string()),
            Self::Localtunnel => trimmed
                .split_whitespace()
                .find(|w| {
                    (w.starts_with("https://") || w.starts_with("http://"))
                        && (w.contains("loca.lt") || w.contains("localtunnel.me"))
                })
                .map(|w| w.trim_end_matches(',').to_string()),
        }
    }
}

pub fn parse_url_any(line: &str) -> Option<String> {
    let trimmed = line.trim();
    for provider in [
        Provider::TryCloudflare,
        Provider::LocalhostRun,
        Provider::Localtunnel,
    ] {
        if let Some(url) = provider.parse_url(trimmed) {
            return Some(url);
        }
    }
    None
}

async fn stream_lines(
    reader: impl tokio::io::AsyncRead + Unpin,
    tx: mpsc::Sender<String>,
    provider: Provider,
    url_found: Arc<Mutex<Option<String>>>,
    _is_stdout: bool,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut last_line = String::new();
    let mut url_sent = false;
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.ok().unwrap_or(0);
        if n == 0 {
            break;
        }
        let trimmed = line.trim_end().to_string();
        if trimmed == last_line {
            continue;
        }
        last_line = trimmed.clone();
        let _ = tx.send(trimmed.clone()).await;
        if !url_sent && url_found.lock().unwrap().is_none() {
            if let Some(u) = provider.parse_url(&trimmed) {
                *url_found.lock().unwrap() = Some(u.clone());
                url_sent = true;
                let _ = tx.send(format!("[URL] {}", u)).await;
            }
        }
    }
}

pub async fn run_tunnel(
    provider: &str,
    port: u16,
    tx: mpsc::Sender<String>,
) -> Result<Option<String>, String> {
    let provider =
        Provider::from_str(provider).ok_or_else(|| format!("Unknown provider: {}", provider))?;

    let bin_name = provider.binary_name();
    let bin_path = match which(bin_name) {
        Ok(p) => p,
        Err(_) => {
            let hint = provider.install_hint().unwrap_or("Install it and retry.");
            return Err(format!("`{}` is not installed. {}", bin_name, hint));
        }
    };

    let mut cmd = Command::new(&bin_path);
    cmd.args(provider.args(port))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start {}: {}", bin_name, e))?;

    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;

    let url_found = Arc::new(Mutex::new(None::<String>));

    let stdout_task = tokio::spawn(stream_lines(
        stdout,
        tx.clone(),
        provider,
        url_found.clone(),
        true,
    ));
    let stderr_task = tokio::spawn(stream_lines(
        stderr,
        tx.clone(),
        provider,
        url_found.clone(),
        false,
    ));

    let _ = futures_util::future::join(stdout_task, stderr_task).await;

    let result = url_found.lock().unwrap().clone();
    let _ = child.wait().await;
    Ok(result)
}

/// Like `run_tunnel`, but stores the spawned `Child` into `child_out` so the
/// caller can keep the process alive after this function returns.
pub async fn run_tunnel_owned(
    provider: &str,
    port: u16,
    tx: mpsc::Sender<String>,
    child_out: Arc<TokioMutex<Option<tokio::process::Child>>>,
) -> Result<Option<String>, String> {
    let provider =
        Provider::from_str(provider).ok_or_else(|| format!("Unknown provider: {}", provider))?;

    let bin_name = provider.binary_name();
    let bin_path = match which(bin_name) {
        Ok(p) => p,
        Err(_) => {
            let hint = provider.install_hint().unwrap_or("Install it and retry.");
            return Err(format!("`{}` is not installed. {}", bin_name, hint));
        }
    };

    let mut cmd = Command::new(&bin_path);
    cmd.args(provider.args(port))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start {}: {}", bin_name, e))?;

    // Store child so caller can keep it alive
    {
        let mut guard = child_out.lock().await;
        *guard = Some(child);
    }

    // We need the child back to read stdout/stderr
    let mut child = child_out
        .lock()
        .await
        .take()
        .ok_or("child missing after spawn")?;

    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;

    let url_found = Arc::new(Mutex::new(None::<String>));

    let stdout_task = tokio::spawn(stream_lines(
        stdout,
        tx.clone(),
        provider,
        url_found.clone(),
        true,
    ));
    let stderr_task = tokio::spawn(stream_lines(
        stderr,
        tx.clone(),
        provider,
        url_found.clone(),
        false,
    ));

    let _ = futures_util::future::join(stdout_task, stderr_task).await;

    let result = url_found.lock().unwrap().clone();

    // Return child to the holder so it stays alive
    {
        let mut guard = child_out.lock().await;
        *guard = Some(child);
    }

    Ok(result)
}

pub async fn install_provider(provider: &str) -> Result<String, String> {
    let provider =
        Provider::from_str(provider).ok_or_else(|| format!("Unknown provider: {}", provider))?;

    match provider {
        Provider::TryCloudflare => Err(
            "Cloudflared must be installed manually. See https://github.com/cloudflare/cloudflared/releases".into(),
        ),
        Provider::LocalhostRun => {
            let os = std::env::consts::OS;
            match os {
                "linux" => {
                    let output = Command::new("sh")
                        .arg("-c")
                        .arg("apt-get install -y openssh-client 2>/dev/null || apk add openssh 2>/dev/null || true")
                        .output()
                        .await
                        .map_err(|e| format!("Failed to run package manager: {}", e))?;
                    let out = String::from_utf8_lossy(&output.stdout).to_string()
                        + String::from_utf8_lossy(&output.stderr).as_ref();
                    if which("ssh").is_ok() {
                        Ok(out)
                    } else {
                        Err("OpenSSH client not found after install attempt. Install it manually via your system package manager.".into())
                    }
                }
                "macos" => {
                    let output = Command::new("sh")
                        .arg("-c")
                        .arg("brew install openssh 2>/dev/null || true")
                        .output()
                        .await
                        .map_err(|e| format!("Failed to run brew: {}", e))?;
                    let out = String::from_utf8_lossy(&output.stdout).to_string()
                        + String::from_utf8_lossy(&output.stderr).as_ref();
                    if which("ssh").is_ok() {
                        Ok(out)
                    } else {
                        Err("OpenSSH client not found. Install it manually: brew install openssh".into())
                    }
                }
                _ => Err("Automatic install of OpenSSH is not supported on this OS. Install it manually.".into()),
            }
        }
        Provider::Localtunnel => {
            let npm = which("npm").map_err(|_| "npm is not installed. Install Node.js (>= 18) first.".to_string())?;
            let output = Command::new(&npm)
                .args(["install", "-g", "localtunnel"])
                .kill_on_drop(true)
                .output()
                .await
                .map_err(|e| format!("Failed to run npm: {}", e))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.is_empty() { stdout } else { format!("{}\n{}", stdout, stderr) };
            if output.status.success() {
                Ok(combined)
            } else {
                Err(combined)
            }
        }
    }
}
