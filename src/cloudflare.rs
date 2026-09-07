//! Automatic Cloudflare Quick Tunnel (trycloudflare.com).
//!
//! On startup the server downloads `cloudflared` if it is not already present,
//! then opens a quick tunnel to the local WebSocket port.  The public URL is
//! printed to stderr so the user can share it or open the app from a remote
//! device without port-forwarding.

use std::io::Write;
use tokio::process::{Child, Command};

// ---------------------------------------------------------------------------
// Binary download
// ---------------------------------------------------------------------------

fn cloudflared_binary_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "cloudflared.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "cloudflared"
    }
}

/// Return the path where we keep the cloudflared binary.
/// `~/.noide/cloudflared` on Unix, `%LOCALAPPDATA%\noide\cloudflared` on Windows.
fn cloudflared_path() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
            format!(
                "{}\\AppData\\Local",
                std::env::var("USERPROFILE").unwrap_or_default()
            )
        });
        std::path::PathBuf::from(base)
            .join("noide")
            .join(cloudflared_binary_name())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        std::path::PathBuf::from(home)
            .join(".noide")
            .join(cloudflared_binary_name())
    }
}

/// Platform triple used in the GitHub release asset name.
fn platform_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-amd64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "darwin-amd64",
        ("macos", "aarch64") => "darwin-arm64",
        ("windows", "x86_64") => "windows-amd64",
        _ => "linux-amd64", // best-effort fallback
    }
}

/// Download the cloudflared binary from GitHub releases if it is not already
/// present at `dest`.  Returns `Ok(path)` on success.
fn ensure_cloudflared(dest: &std::path::Path) -> Result<std::path::PathBuf, String> {
    if dest.exists() {
        return Ok(dest.to_path_buf());
    }

    let triple = platform_triple();
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-{triple}"
    );

    eprintln!("[NoIDE] Downloading cloudflared ({triple})…");

    // Blocking download — fine at startup before the async loop.
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("failed to download cloudflared: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "cloudflared download failed (HTTP {})",
            resp.status()
        ));
    }

    // Write to a temp file first, then atomically rename.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create ~/.noide: {e}"))?;
    }

    let tmp = dest.with_extension("tmp");
    {
        let mut file =
            std::fs::File::create(&tmp).map_err(|e| format!("failed to create temp file: {e}"))?;
        let bytes = resp
            .bytes()
            .map_err(|e| format!("failed to read download: {e}"))?;
        file.write_all(&bytes)
            .map_err(|e| format!("failed to write binary: {e}"))?;
    }

    std::fs::rename(&tmp, dest).map_err(|e| format!("failed to install cloudflared: {e}"));

    // Make executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(dest, perms)
            .map_err(|e| format!("failed to chmod cloudflared: {e}"))?;
    }

    eprintln!("[NoIDE] cloudflared installed to {}", dest.display());
    Ok(dest.to_path_buf())
}

// ---------------------------------------------------------------------------
// Tunnel management
// ---------------------------------------------------------------------------

/// Start a Cloudflare Quick Tunnel for `port` and return the child process
/// handle (so the caller can kill it on shutdown).
///
/// The tunnel URL is printed to stderr once it becomes available.
pub async fn start_tunnel(port: u16) -> Result<(String, Child), String> {
    let bin = ensure_cloudflared(&cloudflared_path())?;

    eprintln!("[NoIDE] Starting Cloudflare tunnel to port {port}…");

    let mut child = Command::new(&bin)
        .args(["tunnel", "--url", &format!("http://127.0.0.1:{port}")])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to start cloudflared: {e}"))?;

    // Read stderr line-by-line to find the trycloudflare.com URL.
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "cloudflared: failed to capture stderr".to_string())?;

    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(stderr).lines();
    let url = loop {
        let line = reader
            .next_line()
            .await
            .map_err(|e| format!("cloudflared stderr read error: {e}"))?
            .ok_or_else(|| "cloudflared exited before producing a tunnel URL".to_string())?;

        if line.contains("trycloudflare.com") {
            if let Some(url) = line
                .split_whitespace()
                .find(|w| w.starts_with("https://") && w.contains("trycloudflare.com"))
            {
                break url.to_string();
            }
        }
    };

    Ok((url, child))
}
