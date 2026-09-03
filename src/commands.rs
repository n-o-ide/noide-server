#![allow(dead_code)]

// Many items in this module are part of the wire protocol's API surface
// (fields the client sends, helpers kept for parity with the documented
// command set) even when the current dispatch path doesn't touch every one
// of them. Previously the crate was a library (Tauri), where `pub` items are
// never flagged as dead; as a standalone binary they would warn — silence
// module-locally instead of deleting protocol-shaped code.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: String,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    pub modified: Vec<String>,
    pub staged: Vec<String>,
    pub untracked: Vec<String>,
    pub deleted: Vec<String>,
    /// Per-file status letter for the staged column (A/M/D/R/C/T)
    #[serde(default)]
    pub staged_details: std::collections::BTreeMap<String, char>,
    /// Per-file status letter for the worktree column (M/D/U)
    #[serde(default)]
    pub worktree_details: std::collections::BTreeMap<String, char>,
    /// Files with an unmerged index entry (merge conflicts).
    #[serde(default)]
    pub unmerged: Vec<String>,
    /// Per-file added/deleted lines in the worktree (unstaged).
    #[serde(default)]
    pub worktree_stats: std::collections::BTreeMap<String, GitFileStat>,
    /// Per-file added/deleted lines staged in the index.
    #[serde(default)]
    pub staged_stats: std::collections::BTreeMap<String, GitFileStat>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStat {
    pub insertions: u32,
    pub deletions: u32,
}

/// Parse `git diff --numstat` output ("adds\tdels\tpath") into a per-file map.
fn parse_numstat(path: &str, args: &[&str]) -> std::collections::BTreeMap<String, GitFileStat> {
    let mut map = std::collections::BTreeMap::new();
    let out = run_git(path, args).unwrap_or_default();
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(adds), Some(dels), Some(file)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        map.insert(
            file.trim().to_string(),
            GitFileStat {
                insertions: adds.parse().unwrap_or(0),
                deletions: dels.parse().unwrap_or(0),
            },
        );
    }
    map
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitDiff {
    pub file: String,
    pub content: String,
}

pub fn run_command(command: String) -> Result<String, String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .map_err(|e| format!("Failed to execute command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !stderr.is_empty() {
        Ok(format!("{}\n{}", stdout, stderr))
    } else {
        Ok(stdout)
    }
}

pub fn run_git(path: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        let combined = format!("{}{}", stdout, stderr);
        Ok(combined.trim().to_string())
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr }
            .trim()
            .to_string())
    }
}

pub fn git_init(path: String) -> Result<String, String> {
    run_git(&path, &["init"])
}

fn parse_branch_head(head: &str) -> (String, u32, u32) {
    let info = head.trim_start_matches('#').trim();
    // "## No commits yet on main"
    if info.starts_with("No commits yet") {
        let branch = info.rsplit("on ").next().unwrap_or("main").to_string();
        return (branch, 0, 0);
    }
    let branch = info.split("...").next().unwrap_or("").trim().to_string();
    let mut ahead = 0u32;
    let mut behind = 0u32;
    if let Some(start) = info.find('[') {
        if let Some(end) = info.find(']') {
            for part in info[start + 1..end].split(',') {
                let p = part.trim();
                if let Some(n) = p.strip_prefix("ahead ") {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = p.strip_prefix("behind ") {
                    behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
    (branch, ahead, behind)
}

pub fn get_git_status(path: String) -> Result<GitStatus, String> {
    // Detect a missing repository so the UI can offer `git init`.
    let probe = Command::new("git")
        .arg("-C")
        .arg(&path)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;
    if !probe.status.success() {
        return Err("not a git repository".to_string());
    }

    let status_output = run_git(&path, &["status", "-b", "--porcelain"])?;
    let mut lines = status_output.lines();
    let (branch, ahead, behind) = parse_branch_head(lines.next().unwrap_or(""));

    // Authoritative conflict list: any file with an unmerged index entry.
    let unmerged: Vec<String> = run_git(&path, &["diff", "--name-only", "--diff-filter=U"])
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut staged = Vec::new();
    let mut modified = Vec::new();
    let mut untracked = Vec::new();
    let mut deleted = Vec::new();
    let mut staged_details = std::collections::BTreeMap::new();
    let mut worktree_details = std::collections::BTreeMap::new();

    for line in lines {
        if line.len() < 4 {
            continue;
        }
        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let file = line[3..].to_string();

        // Unmerged files are surfaced separately and not double-counted into
        // staged/modified/untracked.
        if unmerged.iter().any(|f| f == &file) {
            staged_details.insert(file.clone(), x);
            worktree_details.insert(file.clone(), y);
            continue;
        }

        if x == '?' && y == '?' {
            untracked.push(file.clone());
            worktree_details.insert(file, 'U');
            continue;
        }
        if x != ' ' {
            staged.push(file.clone());
            let letter = match x {
                'A' => 'A',
                'D' => 'D',
                'R' => 'R',
                'C' => 'C',
                'T' => 'T',
                _ => 'M',
            };
            staged_details.insert(file.clone(), letter);
            if x == 'D' {
                deleted.push(file.clone());
            }
        }
        if y != ' ' {
            if y == 'D' {
                deleted.push(file.clone());
                worktree_details.insert(file, 'D');
            } else {
                modified.push(file.clone());
                worktree_details.insert(file, 'M');
            }
        }
    }

    Ok(GitStatus {
        branch,
        ahead,
        behind,
        modified,
        staged,
        untracked,
        deleted,
        staged_details,
        worktree_details,
        unmerged,
        worktree_stats: parse_numstat(&path, &["diff", "--numstat"]),
        staged_stats: parse_numstat(&path, &["diff", "--cached", "--numstat"]),
    })
}

pub fn get_git_diff(path: String, file: String) -> Result<String, String> {
    run_git(&path, &["diff", "--no-color", "--", &file])
}

pub fn get_git_staged_diff(path: String, file: String) -> Result<String, String> {
    run_git(&path, &["diff", "--cached", "--no-color", "--", &file])
}

/// Content of a file at HEAD (for computing editor change highlights).
pub fn get_git_head_file(path: String, file: String) -> Result<String, String> {
    run_git(&path, &["show", &format!("HEAD:{}", file)])
}

/// Content of a file as staged in the index (`git show :file`). Returns an
/// empty string when the path has no index entry (e.g. untracked/new files)
/// so the frontend can render it as a blank baseline for diffs.
pub fn git_show_index(path: String, file: String) -> Result<String, String> {
    match run_git(&path, &["show", &format!(":{}", file)]) {
        Ok(v) => Ok(v),
        Err(_) => Ok(String::new()),
    }
}

pub fn git_add(path: String, file: String) -> Result<String, String> {
    run_git(&path, &["add", "--", &file])
}

pub fn git_stage_all(path: String) -> Result<String, String> {
    run_git(&path, &["add", "-A", "."])
}

pub fn git_unstage(path: String, file: String) -> Result<String, String> {
    match run_git(&path, &["reset", "HEAD", "--", &file]) {
        Ok(v) => Ok(v),
        // Repos without an initial commit cannot reset; use rm --cached.
        Err(_) => run_git(&path, &["rm", "--cached", "--", &file]),
    }
}

pub fn git_unstage_all(path: String) -> Result<String, String> {
    match run_git(&path, &["reset", "HEAD"]) {
        Ok(v) => Ok(v),
        Err(_) => run_git(&path, &["rm", "-r", "--cached", "."]),
    }
}

/// VS Code-style discard: checkout tracked files, delete untracked ones.
pub fn git_discard(path: String, file: String) -> Result<String, String> {
    let tracked = run_git(&path, &["ls-files", "--error-unmatch", "--", &file]).is_ok();
    if tracked {
        return run_git(&path, &["checkout", "--", &file]);
    }
    let full = Path::new(&path).join(&file);
    if file.ends_with('/') || full.is_dir() {
        fs::remove_dir_all(&full).map_err(|e| format!("Failed to delete {}: {}", file, e))?;
    } else if full.is_file() {
        fs::remove_file(&full).map_err(|e| format!("Failed to delete {}: {}", file, e))?;
    }
    Ok(String::new())
}

/// Discard ALL changes: unstage, hard-reset to HEAD, and remove untracked
/// files/directories. Ignored files are left alone (like VS Code).
/// Works on repositories without an initial commit.
pub fn git_discard_all(path: String) -> Result<String, String> {
    let has_head = run_git(&path, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();

    // Unstage everything so staged entries become part of the worktree sweep.
    run_git(&path, &["reset"])?;

    if has_head {
        run_git(&path, &["reset", "--hard", "HEAD"])?;
    }

    // Remove untracked files and directories (not ignored ones).
    run_git(&path, &["clean", "-fd"])
}

pub fn git_commit(path: String, message: String, amend: Option<bool>) -> Result<String, String> {
    if amend.unwrap_or(false) {
        if message.trim().is_empty() {
            // Keep the existing commit message.
            return run_git(&path, &["commit", "--amend", "--no-edit"]);
        }
        return run_git(&path, &["commit", "--amend", "-m", &message]);
    }
    run_git(&path, &["commit", "-m", &message])
}

/// Undo the last commit, keeping its changes staged (soft reset).
pub fn git_undo_commit(path: String) -> Result<String, String> {
    run_git(&path, &["reset", "--soft", "HEAD~1"])
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    /// Commit timestamp (unix seconds).
    pub time: i64,
    pub insertions: u32,
    pub deletions: u32,
}

/// Recent commit history with per-commit insertion/deletion totals.
pub fn git_log(path: String, limit: Option<usize>) -> Result<Vec<GitCommitInfo>, String> {
    // Repos without an initial commit have no history.
    let has_head = run_git(&path, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    if !has_head {
        return Ok(Vec::new());
    }
    let n = limit.unwrap_or(50).to_string();
    let out = run_git(
        &path,
        &[
            "log",
            "--no-color",
            &format!("-{}", n),
            // '@' starts a record; \x1f separates fields (never in subjects).
            "--pretty=format:@%H\u{1f}%h\u{1f}%an\u{1f}%at\u{1f}%s",
            "--numstat",
        ],
    )?;

    let mut commits: Vec<GitCommitInfo> = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix('@') {
            let mut f = rest.split('\u{1f}');
            let (Some(hash), Some(short_hash), Some(author), Some(time), Some(message)) =
                (f.next(), f.next(), f.next(), f.next(), f.next())
            else {
                continue;
            };
            commits.push(GitCommitInfo {
                hash: hash.to_string(),
                short_hash: short_hash.to_string(),
                author: author.to_string(),
                message: message.to_string(),
                time: time.parse().unwrap_or(0),
                insertions: 0,
                deletions: 0,
            });
        } else {
            // numstat line: "adds\tdels\tpath"
            let mut parts = line.splitn(3, '\t');
            if let (Some(a), Some(d), Some(_)) = (parts.next(), parts.next(), parts.next()) {
                if let Some(c) = commits.last_mut() {
                    c.insertions += a.parse::<u32>().unwrap_or(0);
                    c.deletions += d.parse::<u32>().unwrap_or(0);
                }
            }
        }
    }
    Ok(commits)
}

/// Unified diff of a single commit (`git show`).
pub fn git_commit_diff(path: String, commit: String) -> Result<String, String> {
    let commit = commit.trim().to_string();
    if commit.is_empty() || commit.starts_with('-') {
        return Err("Invalid commit".to_string());
    }
    run_git(&path, &["show", "--no-color", "--no-ext-diff", &commit])
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStashInfo {
    #[serde(rename = "ref")]
    pub reference: String,
    pub message: String,
    /// Stash timestamp (unix seconds).
    pub time: i64,
}

pub fn git_stash_list(path: String) -> Result<Vec<GitStashInfo>, String> {
    let out =
        run_git(&path, &["stash", "list", "--format=%gd\u{1f}%at\u{1f}%gs"]).unwrap_or_default();
    let mut stashes = Vec::new();
    for line in out.lines() {
        let mut f = line.split('\u{1f}');
        let (Some(r), Some(time), Some(message)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        stashes.push(GitStashInfo {
            reference: r.to_string(),
            message: message.to_string(),
            time: time.parse().unwrap_or(0),
        });
    }
    Ok(stashes)
}

/// Stash all changes, including untracked files.
pub fn git_stash(path: String) -> Result<String, String> {
    run_git(&path, &["stash", "push", "-u", "-m", "WIP from NoIDE"])
}

/// Pop the most recent stash, restoring its changes.
pub fn git_stash_pop(path: String) -> Result<String, String> {
    run_git(&path, &["stash", "pop"])
}

pub fn git_push(path: String) -> Result<String, String> {
    run_git(&path, &["push"])
}

pub fn git_fetch(path: String) -> Result<String, String> {
    run_git(&path, &["fetch", "--all", "--prune"])
}

pub fn git_pull(path: String) -> Result<String, String> {
    run_git(&path, &["pull"])
}

pub fn git_revert(path: String, commit: String) -> Result<String, String> {
    run_git(&path, &["revert", "--no-edit", &commit])
}

/// Start a merge of `from_ref` into the current branch. Conflicts produce a
/// non-zero exit code, but that is expected here — the combined output is
/// returned either way so the frontend can refresh the status and surface the
/// conflicted (`unmerged`) files for resolution.
pub fn git_merge(path: String, from_ref: String) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["merge", "--no-edit", &from_ref])
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(format!("{}{}", stdout, stderr).trim().to_string())
}

/// Abort an in-progress merge, restoring the pre-merge state.
pub fn git_merge_abort(path: String) -> Result<String, String> {
    run_git(&path, &["merge", "--abort"])
}

/// Resolve a conflicted file by taking one side and staging it. `side` is
/// "ours" (current branch) or "theirs" (the branch being merged in).
pub fn git_resolve_conflict(path: String, file: String, side: String) -> Result<String, String> {
    let flag = if side == "theirs" {
        "--theirs"
    } else {
        "--ours"
    };
    run_git(&path, &["checkout", flag, "--", &file])?;
    run_git(&path, &["add", "--", &file])
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranches {
    pub current: String,
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
}

pub fn git_branch_list(path: String) -> Result<GitBranches, String> {
    let local_out = run_git(
        &path,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    let branches: Vec<String> = local_out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let remote_out = run_git(
        &path,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )
    .unwrap_or_default();
    let remotes: Vec<String> = remote_out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.ends_with("/HEAD"))
        .collect();

    let current = run_git(&path, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();

    Ok(GitBranches {
        current,
        branches,
        remotes,
    })
}

/// Switch branches; with create=true this becomes `git checkout -b`.
/// Remote-tracking selections ("origin/foo") create a local tracking
/// branch, matching VS Code behaviour.
pub fn git_checkout_branch(
    path: String,
    name: String,
    create: Option<bool>,
) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.contains("..") || name.starts_with('-') {
        return Err("Invalid branch name".to_string());
    }
    if create.unwrap_or(false) {
        return run_git(&path, &["checkout", "-b", &name]);
    }

    fn verify_ref(path: &str, full: &str) -> bool {
        run_git(
            path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{}^{{commit}}", full),
            ],
        )
        .is_ok()
    }

    // Create a local branch from a starting point; if tracking info cannot
    // be set up (e.g. synthetic refs), fall back to a plain new branch.
    fn track_from(path: &str, local: &str, start: &str) -> Result<String, String> {
        match run_git(path, &["checkout", "-b", local, "--track", start]) {
            Ok(v) => Ok(v),
            Err(track_err) => match run_git(path, &["checkout", "-b", local, start]) {
                Ok(v) => Ok(v),
                Err(_) => Err(track_err),
            },
        }
    }

    // 1. Explicitly remote-prefixed selection ("origin/foo"): never detach;
    //    create/switch to the local branch instead.
    if name.contains('/') && verify_ref(&path, &format!("refs/remotes/{}", name)) {
        let local = name.split('/').skip(1).collect::<Vec<_>>().join("/");
        if !local.is_empty() {
            return track_from(&path, &local, &name);
        }
    }

    // 2. Plain selection: normal checkout first…
    match run_git(&path, &["checkout", &name]) {
        Ok(v) => Ok(v),
        Err(plain_err) => {
            // 3. …then fall back to creating a local branch from a remote one
            //    ("foo" from refs/remotes/origin/foo or refs/remotes/foo).
            let mut candidates = Vec::new();
            if name.contains('/') {
                candidates.push(name.clone());
            }
            candidates.push(format!("origin/{}", name));

            for full in candidates {
                if verify_ref(&path, &full) {
                    return track_from(&path, &name, &full);
                }
            }
            Err(plain_err)
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub children: Option<Vec<FileNode>>,
    pub extension: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub modified: Option<u64>,
}

// ===== File tree mutations =====

pub fn create_file(path: String, content: String) -> Result<(), String> {
    let p = Path::new(&path);
    if p.exists() {
        return Err("File already exists".to_string());
    }
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent dir: {}", e))?;
    }
    fs::write(p, content).map_err(|e| format!("Failed to write file: {}", e))
}

pub fn create_directory(path: String) -> Result<(), String> {
    if Path::new(&path).exists() {
        return Err("Folder already exists".to_string());
    }
    fs::create_dir_all(&path).map_err(|e| format!("Failed to create folder: {}", e))
}

pub fn rename_entry(from: String, to: String) -> Result<(), String> {
    if !Path::new(&from).exists() {
        return Err("Source does not exist".to_string());
    }
    if Path::new(&to).exists() {
        return Err("Target already exists".to_string());
    }
    if let Some(parent) = Path::new(&to).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent dir: {}", e))?;
    }
    fs::rename(&from, &to).map_err(|e| format!("Rename failed: {}", e))
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
        Ok(())
    }
}

pub fn copy_entry(from: String, to: String) -> Result<(), String> {
    let src = Path::new(&from);
    let dst = Path::new(&to);
    if !src.exists() {
        return Err("Source does not exist".to_string());
    }
    if dst.exists() {
        return Err("Target already exists".to_string());
    }
    copy_recursive(src, dst).map_err(|e| format!("Copy failed: {}", e))
}

pub fn delete_entry(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("Path does not exist".to_string());
    }
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| format!("Delete failed: {}", e))
    } else {
        fs::remove_file(p).map_err(|e| format!("Delete failed: {}", e))
    }
}

pub fn read_file(path: String) -> Result<String, String> {
    fs::read_to_string(&path).map_err(|e| format!("Failed to read file: {}", e))
}

/// Binary-safe read for images etc. Returns standard base64.
pub fn read_file_base64(path: String) -> Result<String, String> {
    let bytes = fs::read(&path).map_err(|e| format!("Failed to read file: {}", e))?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn write_file(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }
    fs::write(&path, &content).map_err(|e| format!("Failed to write file: {}", e))
}

/// Write a file from a base64-encoded content string.
pub fn upload_file(path: String, content: String) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&content)
        .map_err(|e| format!("Invalid base64: {}", e))?;
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }
    fs::write(&path, &bytes).map_err(|e| format!("Failed to write file: {}", e))
}

/// Return a file as base64 for download.
pub fn download_file(path: String) -> Result<String, String> {
    let bytes = fs::read(&path).map_err(|e| format!("Failed to read file for download: {}", e))?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Extract a zip archive (base64-encoded) into the given directory.
pub fn extract_zip(zip_content_b64: String, dest_dir: String) -> Result<Vec<String>, String> {
    use base64::Engine;
    let zip_bytes = base64::engine::general_purpose::STANDARD
        .decode(&zip_content_b64)
        .map_err(|e| format!("Invalid base64: {}", e))?;
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open zip: {}", e))?;
    fs::create_dir_all(&dest_dir).map_err(|e| format!("Failed to create destination: {}", e))?;
    let mut extracted = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry: {}", e))?;
        let outpath = Path::new(&dest_dir).join(entry.mangled_name());
        if entry.is_dir() {
            fs::create_dir_all(&outpath).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {}", e))?;
            }
            let mut outfile =
                fs::File::create(&outpath).map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("Failed to write file: {}", e))?;
        }
        extracted.push(outpath.to_string_lossy().to_string());
    }
    Ok(extracted)
}

pub fn read_directory(path: String, max_depth: Option<u32>) -> Result<Vec<FileNode>, String> {
    let depth = max_depth.unwrap_or(3);
    read_dir_recursive(&path, depth, 0)
}

fn read_dir_recursive(
    path: &str,
    max_depth: u32,
    current_depth: u32,
) -> Result<Vec<FileNode>, String> {
    let entries = fs::read_dir(path).map_err(|e| format!("Failed to read directory: {}", e))?;
    let mut nodes = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = entry.path().to_string_lossy().to_string();
        // Prefer the cheap dirent type (populated by readdir, no extra stat
        // syscall) so listing large directories is fast. Only fall back to a
        // stat for symlinks (which must be resolved) and for the rare
        // DT_UNKNOWN case where the filesystem doesn't report a type.
        let is_directory = match entry.file_type() {
            Ok(ft) if ft.is_dir() => true,
            Ok(ft) if ft.is_symlink() => fs::metadata(&entry_path)
                .map(|m| m.is_dir())
                .unwrap_or(false),
            Ok(ft) if ft.is_file() => false,
            Ok(_) => fs::metadata(&entry_path)
                .map(|m| m.is_dir())
                .unwrap_or(false),
            Err(_) => false,
        };

        if name == ".git" || name == "node_modules" || name == "target" || name == "dist" {
            continue;
        }

        let extension = if is_directory {
            None
        } else {
            Path::new(&name)
                .extension()
                .map(|e| e.to_string_lossy().to_string())
        };

        let children = if is_directory && current_depth < max_depth {
            Some(read_dir_recursive(
                &entry_path,
                max_depth,
                current_depth + 1,
            )?)
        } else {
            None
        };

        let (size, modified) = if let Ok(meta) = entry.metadata() {
            let m = meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            });
            (Some(meta.len()), m)
        } else {
            (None, None)
        };
        nodes.push(FileNode {
            name,
            path: entry_path,
            is_directory,
            children,
            extension,
            size,
            modified,
        });
    }

    nodes.sort_by(|a, b| {
        if a.is_directory == b.is_directory {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        } else if a.is_directory {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });

    Ok(nodes)
}

pub fn file_exists(path: String) -> Result<bool, String> {
    Ok(Path::new(&path).exists())
}

// ===== Search =====

const WALK_IGNORES: [&str; 6] = [".git", "node_modules", "target", "dist", "build", ".noTerm"];
const SEARCH_MAX_RESULTS: usize = 500;
const MAX_FILE_BYTES: usize = 1_000_000;

fn is_ignored_name(name: &str) -> bool {
    WALK_IGNORES.contains(&name)
}

fn walk_files(dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    fn rec(dir: &Path, depth: usize, out: &mut Vec<String>) -> std::io::Result<()> {
        if depth > 12 || out.len() > 20_000 {
            return Ok(());
        }
        // Skip directories we cannot read instead of failing the whole walk.
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if is_ignored_name(&name) {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                rec(&p, depth + 1, out)?;
            } else {
                out.push(p.to_string_lossy().to_string());
            }
        }
        Ok(())
    }
    rec(dir, 0, out).map_err(|e| format!("Failed to walk directory: {}", e))
}

/// Full recursive file listing (absolute paths) for the fuzzy finder.
pub fn list_files(path: String) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    walk_files(Path::new(&path), &mut files)?;
    files.sort();
    Ok(files)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub file: String,
    pub line_number: u32,
    pub line_text: String,
    pub match_start: u32,
    pub match_end: u32,
}

/// Server-side content search across the whole tree (like VS Code's
/// global search). Returns character-offset matches per line.
pub fn search_in_files(
    path: String,
    query: String,
    case_sensitive: Option<bool>,
    whole_word: Option<bool>,
    regex: Option<bool>,
) -> Result<Vec<SearchMatch>, String> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }

    let use_regex = regex.unwrap_or(false);
    let mut pattern = if use_regex {
        query.clone()
    } else {
        regex::escape(&query)
    };
    if whole_word.unwrap_or(false) {
        pattern = format!(r"\b(?:{})\b", pattern);
    }
    if !case_sensitive.unwrap_or(false) {
        pattern = format!("(?i){}", pattern);
    }
    let re = regex::Regex::new(&pattern).map_err(|e| format!("Invalid search: {}", e))?;

    let mut files = Vec::new();
    walk_files(Path::new(&path), &mut files)?;

    let mut matches = Vec::new();

    'files: for f in &files {
        let bytes = match fs::read(f) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Skip binaries (NUL byte) and very large files.
        if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
            continue;
        }
        let content = String::from_utf8_lossy(&bytes);
        // Absolute paths so the frontend can open results directly.
        let rel = f.clone();

        for (idx, line) in content.lines().enumerate() {
            for m in re.find_iter(line) {
                // Convert byte offsets to character offsets for the frontend.
                let start_char = line[..m.start()].chars().count() as u32;
                let end_char = line[..m.end()].chars().count() as u32;
                matches.push(SearchMatch {
                    file: rel.clone(),
                    line_number: (idx + 1) as u32,
                    line_text: line.to_string(),
                    match_start: start_char,
                    match_end: end_char,
                });
                if matches.len() >= SEARCH_MAX_RESULTS {
                    break 'files;
                }
            }
        }
    }

    Ok(matches)
}

// ===== Chat =====

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatModel {
    pub id: String,
    pub label: String,
}

/// Full OpenAI-compatible `/models` endpoint for each agent's own model API.
/// Each agent exposes its own endpoint; these are intentionally NOT derived
/// from any local config file (e.g. providers.json).
fn agent_models_url(agent: &str) -> Option<&'static str> {
    // Accept both "opencode"/"kilo" and the hyphenated "open-code"/"kilo-code"
    // forms used by older callers/tests.
    match agent.trim().replace('-', "") {
        s if s.eq_ignore_ascii_case("opencode") => Some("https://openrouter.ai/api/v1/models"),
        s if s.eq_ignore_ascii_case("kilo") => Some("https://api.kilo.ai/api/gateway/models"),
        _ => None,
    }
}

/// Query an agent's OpenAI-compatible `/models` endpoint over HTTP and return
/// the model ids. Uses the async `reqwest::Client` because this runs inside the
/// tokio runtime (the WebSocket handler task); a blocking client would panic.
async fn query_agent_models(models_url: &str, api_key: &str) -> Result<Vec<ChatModel>, String> {
    // Config values sometimes contain stray whitespace; normalise the URL.
    let url: String = models_url.split_whitespace().collect();
    let url = url.trim_end_matches('/');
    if url.is_empty() {
        return Err("Empty model API URL".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut req = client.get(url).header("Content-Type", "application/json");
    // Some agents expose a public `/models` endpoint; only send the bearer
    // token when one is actually configured in the chat panel.
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Request to {} failed: {}", url, e))?;

    if !resp.status().is_success() {
        return Err(format!("{} returned status {}", url, resp.status()));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response from {}: {}", url, e))?;

    let data = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| format!("No 'data' array in response from {}", url))?;

    let models: Vec<ChatModel> = data
        .iter()
        .filter_map(|m| {
            m.get("id").and_then(|id| id.as_str()).map(|id| ChatModel {
                id: id.to_string(),
                label: id.to_string(),
            })
        })
        .collect();

    if models.is_empty() {
        Err(format!("No models returned from {}", url))
    } else {
        Ok(models)
    }
}

/// Return the list of models for an agent by querying the agent's own
/// OpenAI-compatible `/v1/models` endpoint directly. The API key is supplied by
/// the client (the chat panel's API-key field), not read from local config.
pub async fn chat_models(agent: String, api_key: String) -> Result<Vec<ChatModel>, String> {
    let url = agent_models_url(agent.trim()).ok_or_else(|| format!("Unknown agent '{}'", agent))?;

    query_agent_models(url, &api_key).await
}

/// Result of probing whether an agent CLI is usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliStatus {
    /// The executable exists on PATH and its `--version` output identifies it
    /// as the expected agent (kilo / opencode).
    Available,
    /// No executable with this name was found on PATH.
    Missing,
    /// An executable with this name exists on PATH, but its `--version` output
    /// does not identify it as the expected agent (e.g. a shadowing binary).
    WrongBinary,
}

impl CliStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CliStatus::Available => "available",
            CliStatus::Missing => "missing",
            CliStatus::WrongBinary => "wrong",
        }
    }
}

/// Token used to verify the binary's identity in `--version` output.
fn agent_token(command: &str) -> &str {
    command
}

/// Directories to search for an agent CLI when it isn't resolvable via the
/// current PATH. GUI/launcher processes often don't inherit shell PATH
/// customizations (e.g. an nvm-managed `node` that puts `kilo` on PATH), so
/// we also probe the usual install locations before reporting the CLI missing.
fn agent_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        for p in std::env::split_paths(&path) {
            dirs.push(p);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = std::path::PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            for entry in entries.flatten() {
                let bin = entry.path().join("bin");
                if bin.is_dir() {
                    dirs.push(bin);
                }
            }
        }
    }
    dirs
}

/// Resolve the path to an agent's executable.
///
/// Returns `None` only when no usable binary can be found on PATH or in the
/// common install locations we probe. When found, the returned path is used
/// directly so callers don't depend on the spawning process's PATH.
///
/// This is a pure filesystem lookup (stat calls only) — no process is
/// spawned, so it's safe to call on every chat message.
pub fn resolve_agent_bin(command: &str) -> Option<std::path::PathBuf> {
    // Check PATH directories and common install locations (nvm, ~/.local/bin).
    for dir in agent_search_dirs() {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Probe whether the given agent CLI is installed and usable.
///
/// We run `<command> --version` and inspect the result:
///   * `NotFound` / no binary anywhere     -> `Missing`
///   * the binary runs (ideally names the agent) -> `Available`
///   * other spawn error                   -> treat as available (avoid false negatives)
///
/// The identity token (`kilo`, `opencode`) is only a *positive* hint. Some
/// agents print a bare version (e.g. `kilo --version` => `7.4.23`) that does
/// not contain the token, so a missing token must NOT be treated as "not
/// installed" — that produced false "not on your PATH" warnings. Any binary
/// that resolves and runs `--version` is therefore treated as available.
///
/// Note: this only verifies the binary exists and runs, not that the user is
/// authenticated or that the agent can reach the network. Credential/network
/// failures surface later, when the command is actually executed.
pub fn chat_cli_check(command: &str) -> CliStatus {
    let exe = match resolve_agent_bin(command) {
        Some(e) => e,
        None => return CliStatus::Missing,
    };

    let token = agent_token(command);
    match Command::new(&exe).arg("--version").output() {
        Ok(output) => {
            let combined = String::from_utf8_lossy(&output.stdout).to_lowercase()
                + &String::from_utf8_lossy(&output.stderr).to_lowercase();
            // Positive identification strengthens confidence but is optional.
            let _ = combined.contains(token);
            CliStatus::Available
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CliStatus::Missing,
        Err(_) => CliStatus::Available,
    }
}

/// Returns true if the given CLI command is resolvable on the user's PATH
/// AND its `--version` output identifies it as the expected agent.
pub fn chat_cli_available(command: &str) -> bool {
    matches!(chat_cli_check(command), CliStatus::Available)
}

/// Friendly, actionable message shown when an agent's CLI is not installed.
pub fn cli_missing_message(command: &str) -> String {
    let name = match command {
        "kilo" => "Kilo Code",
        "opencode" => "OpenCode",
        other => other,
    };
    format!(
        "{} CLI (`{}`) was not found on your system PATH. Install it and make sure `{}` is available on your PATH, then retry.",
        name, command, command
    )
}

/// Friendly message shown when the binary exists on PATH but doesn't look like
/// the expected agent.
pub fn cli_wrong_binary_message(command: &str) -> String {
    let name = match command {
        "kilo" => "Kilo Code",
        "opencode" => "OpenCode",
        other => other,
    };
    format!(
        "A binary named `{}` was found on your PATH, but it does not appear to be {} (`{} --version` produced unrecognizable output). \
         Make sure `{}` is the correct agent CLI, then retry.",
        command, name, command, command
    )
}

/// A single attachment handed to `chat_stream` by the frontend. The content is
/// delivered in-memory (the web `File` API has no real path), so the backend
/// writes it to a temporary file and passes it to the agent CLI via its native
/// `-f/--file` flag rather than inlining megabytes of base64 into the prompt
/// argument (which would blow past the OS `ARG_MAX` limit and fail the spawn).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAttachmentInput {
    /// Original filename (e.g. "main.rs").
    pub name: String,
    /// MIME type (e.g. "text/plain", "image/png").
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Base64-encoded content (with optional `data:<mime>;base64,` prefix) for
    /// images / binary files.
    #[serde(default)]
    pub base64: Option<String>,
    /// Raw text content for text files.
    #[serde(default)]
    pub content: Option<String>,
}

/// Write attachment contents to temporary files on disk.
///
/// Returns the paths of every file written. The caller is responsible for
/// deleting them once the agent CLI has finished reading them. Attachments
/// without inline content (path-only references) are skipped — there is
/// nothing to materialize on disk.
pub fn write_attachment_files(
    attachments: &[ChatAttachmentInput],
) -> Result<Vec<std::path::PathBuf>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::with_capacity(attachments.len());
    let base = std::env::temp_dir();
    for (i, att) in attachments.iter().enumerate() {
        let data: Vec<u8> = if let Some(b64) = &att.base64 {
            // Strip an optional `data:<mime>;base64,` prefix.
            let b = b64.split(',').next_back().unwrap_or(b64.as_str());
            base64::engine::general_purpose::STANDARD
                .decode(b)
                .map_err(|e| format!("Failed to decode attachment '{}': {}", att.name, e))?
        } else if let Some(text) = &att.content {
            text.clone().into_bytes()
        } else {
            // Reference-only attachment — nothing to write.
            continue;
        };

        // Derive an extension from the original name so the CLI can guess the
        // file type; fall back to no extension when the name has none.
        let ext = std::path::Path::new(&att.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();
        // Sanitize the name (drop path separators) and make it unique per
        // process + index to avoid collisions across concurrent requests.
        let safe_name = att.name.replace([std::path::MAIN_SEPARATOR, '/'], "_");
        let tmp_name = format!(
            "noide-att-{}-{}-{}{}",
            std::process::id(),
            i,
            safe_name,
            ext
        );
        let tmp_path = base.join(tmp_name);
        std::fs::write(&tmp_path, &data)
            .map_err(|e| format!("Failed to write attachment '{}': {}", att.name, e))?;
        paths.push(tmp_path);
    }
    Ok(paths)
}

/// Remove the temporary attachment files written by `write_attachment_files`.
/// Best-effort: failures are logged but never surface to the caller.
pub fn cleanup_attachment_files(paths: &[std::path::PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

/// Append `-f <path>` flags for each attachment temp file AFTER the trailing
/// prompt argument.
///
/// IMPORTANT: the message positional must come *before* the `-f` flags. kilo's
/// `-f/--file` is an array option that greedily consumes following positionals
/// as file arguments, so placing `-f` after the prompt (e.g.
/// `kilo run "<msg>" -f file`) is required — the reverse order makes kilo treat
/// the message text as a file path ("File not found: <prompt>").
pub fn inject_attachment_args(args: &mut Vec<String>, attachment_paths: &[std::path::PathBuf]) {
    if attachment_paths.is_empty() {
        return;
    }
    // The prompt is the final positional argument; keep it in place and append
    // the attachment flags after it so the message precedes `-f`.
    let prompt = args.pop();
    if let Some(p) = prompt {
        args.push(p);
    }
    for p in attachment_paths {
        args.push("-f".to_string());
        args.push(p.to_string_lossy().to_string());
    }
}

/// Run a CLI command and return its stdout line by line.
/// The frontend calls this via `invokeCommand('chat_stream', { command, args })`
/// and reads the result as a single string (all lines joined).
pub fn chat_stream(
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    api_key: String,
    mode: Option<String>,
    attachments: Vec<ChatAttachmentInput>,
) -> Result<String, String> {
    // Fail fast with a clear message if the agent's CLI isn't installed, rather
    // than a raw "Failed to run <cmd>: No such file or directory" error.
    let exe = match resolve_agent_bin(&command) {
        Some(e) => e,
        None => return Err(cli_missing_message(&command)),
    };

    // Materialize attachment contents to temp files so they can be passed via
    // the CLI's native `-f/--file` flag instead of being inlined into the
    // (size-limited) prompt argument. Doing this up front gives a real error
    // instead of a confusing spawn failure if the write fails.
    let attachment_paths = write_attachment_files(&attachments)?;

    let mut args = args;
    inject_attachment_args(&mut args, &attachment_paths);

    let mut cmd = Command::new(&exe);
    cmd.args(&args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // Pass the API key as an env var so the CLI (e.g. opencode) can
    // authenticate with providers (OpenRouter, etc.) even when its own
    // credential store is empty.
    if !api_key.trim().is_empty() {
        cmd.env("OPENROUTER_API_KEY", &api_key);
    }
    // Switch kilo's real mode via an inline config. Seeding the prompt with
    // mode instructions is not enough — kilo's own mode system overrides it.
    // KILO_CONFIG_CONTENT is deep-merged with highest precedence, so this
    // actually switches Code/Ask/Plan. Effort is handled separately by the
    // `--variant` CLI flag. Only kilo reads this env var; opencode switches
    // mode via the `--agent`/`--permissions` CLI flags (set in the frontend
    // args), so we skip it for non-kilo agents.
    if command == "kilo" {
        if let Some(m) = mode.as_ref() {
            let kilo_mode = match m.as_str() {
                "ask" => "ask",
                "plan" => "plan",
                _ => "code", // build (and any unknown) -> code
            };
            cmd.env(
                "KILO_CONFIG_CONTENT",
                format!("{{\"mode\":\"{}\"}}", kilo_mode),
            );
        }
    }
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            // Only a genuinely missing binary should be reported as the
            // "not on your PATH" error. Other failures (e.g. argument list too
            // long) are real and must be surfaced verbatim, not masked.
            cleanup_attachment_files(&attachment_paths);
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(cli_missing_message(&command));
            }
            return Err(format!("Failed to run {}: {}", command, e));
        }
    };

    // The CLI has read the attachment files by now — clean them up.
    cleanup_attachment_files(&attachment_paths);

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(format!(
            "{} exited with code {}: {}",
            command,
            code,
            stderr.trim()
        ));
    }

    Ok(if stderr.is_empty() {
        stdout
    } else {
        format!("{}\n{}", stdout, stderr)
    })
}

/// Return the current working directory of the backend process.
pub fn get_cwd() -> Result<String, String> {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| format!("Failed to get cwd: {}", e))
}
