use axum::{
    extract::{Multipart, State},
    http::{header, Method},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::fs as async_fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tower_http::cors::{Any, CorsLayer};

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct FileEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<String>,
    permissions: Option<String>,
    extension: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LsRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ReadRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
struct WriteRequest {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct MkdirRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
struct RmRequest {
    path: String,
    recursive: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MvRequest {
    source: String,
    destination: String,
}

#[derive(Debug, Deserialize)]
struct CpRequest {
    source: String,
    destination: String,
    recursive: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct StatRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
    path: String,
    query: String,
    max_depth: Option<usize>,
}

#[derive(Debug, Serialize)]
struct StatResult {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<String>,
    permissions: Option<String>,
    extension: Option<String>,
    children: Option<usize>,
}

struct AppState {}

// ── Helpers ────────────────────────────────────────────────────────────────

fn normalize_path(path: &str) -> PathBuf {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(rest)
    } else if path == "~" {
        dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("."))
    } else {
        PathBuf::from(path)
    };
    let mut normalized = PathBuf::new();
    for component in expanded.components() {
        match component {
            std::path::Component::ParentDir => {
                // Prevent escaping root — simply skip ..
            }
            std::path::Component::Normal(c) => normalized.push(c),
            std::path::Component::RootDir => normalized.push("/"),
            std::path::Component::Prefix(prefix) => {
                normalized.push(prefix.as_os_str());
            }
            _ => {}
        }
    }
    normalized
}

fn file_entry_from_path(path: &FsPath) -> Option<FileEntry> {
    let meta = std::fs::metadata(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_string());
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| {
            let datetime: chrono::DateTime<chrono::Utc> = t.into();
            Some(datetime.to_rfc3339())
        });
    let permissions = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Some(format!("{:o}", meta.permissions().mode() & 0o777))
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    Some(FileEntry {
        name,
        path: path.to_string_lossy().to_string(),
        is_dir: meta.is_dir(),
        size: meta.len(),
        modified,
        permissions,
        extension: ext,
    })
}

// ── Handlers ───────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true }))
}

async fn ls_handler(
    Json(input): Json<LsRequest>,
) -> Result<Json<Vec<FileEntry>>, String> {
    let path = normalize_path(&input.path);
    let mut entries = Vec::new();

    let mut read_dir = async_fs::read_dir(&path)
        .await
        .map_err(|e| format!("Failed to read directory: {}", e))?;

    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read entry: {}", e))?
    {
        let metadata = entry
            .metadata()
            .await
            .map_err(|e| format!("Failed to read metadata: {}", e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let ext = entry.path().extension().map(|e| e.to_string_lossy().to_string());
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| {
                let datetime: chrono::DateTime<chrono::Utc> = t.into();
                Some(datetime.to_rfc3339())
            });
        let permissions = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Some(format!("{:o}", metadata.permissions().mode() & 0o777))
            }
            #[cfg(not(unix))]
            {
                None
            }
        };
        entries.push(FileEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified,
            permissions,
            extension: ext,
        });
    }

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Json(entries))
}

async fn read_handler(
    Json(input): Json<ReadRequest>,
) -> Result<Json<serde_json::Value>, String> {
    let path = normalize_path(&input.path);
    if !path.exists() {
        return Err("File not found".to_string());
    }
    if path.is_dir() {
        return Err("Cannot read a directory".to_string());
    }

    let metadata = async_fs::metadata(&path)
        .await
        .map_err(|e| format!("Failed to read metadata: {}", e))?;

    let size = metadata.len();
    let max_text_size = 10 * 1024 * 1024; // 10MB limit for text

    if size > max_text_size {
        return Ok(Json(serde_json::json!({
            "path": path.to_string_lossy(),
            "size": size,
            "binary": true,
            "content": null,
            "message": "File too large to display"
        })));
    }

    let mut file = async_fs::File::open(&path)
        .await
        .map_err(|e| format!("Failed to open file: {}", e))?;

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;

    // Try to detect binary
    let is_binary = content.iter().any(|&b| b == 0);

    if is_binary {
        Ok(Json(serde_json::json!({
            "path": path.to_string_lossy(),
            "size": size,
            "binary": true,
            "content": null,
            "message": "Binary file"
        })))
    } else {
        let text = String::from_utf8_lossy(&content).to_string();
        Ok(Json(serde_json::json!({
            "path": path.to_string_lossy(),
            "size": size,
            "binary": false,
            "content": text
        })))
    }
}

async fn write_handler(
    Json(input): Json<WriteRequest>,
) -> Result<Json<serde_json::Value>, String> {
    let path = normalize_path(&input.path);

    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        async_fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create parent directories: {}", e))?;
    }

    async_fs::write(&path, input.content.as_bytes())
        .await
        .map_err(|e| format!("Failed to write file: {}", e))?;

    Ok(Json(serde_json::json!({ "ok": true, "path": path.to_string_lossy() })))
}

async fn mkdir_handler(
    Json(input): Json<MkdirRequest>,
) -> Result<Json<serde_json::Value>, String> {
    let path = normalize_path(&input.path);
    async_fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("Failed to create directory: {}", e))?;
    Ok(Json(serde_json::json!({ "ok": true, "path": path.to_string_lossy() })))
}

async fn rm_handler(
    Json(input): Json<RmRequest>,
) -> Result<Json<serde_json::Value>, String> {
    let path = normalize_path(&input.path);
    if !path.exists() {
        return Err("Path not found".to_string());
    }

    let recursive = input.recursive.unwrap_or(false);

    if path.is_dir() {
        if recursive {
            async_fs::remove_dir_all(&path)
                .await
                .map_err(|e| format!("Failed to remove directory: {}", e))?;
        } else {
            async_fs::remove_dir(&path)
                .await
                .map_err(|_| "Directory not empty (use recursive: true)".to_string())?;
        }
    } else {
        async_fs::remove_file(&path)
            .await
            .map_err(|e| format!("Failed to remove file: {}", e))?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn mv_handler(
    Json(input): Json<MvRequest>,
) -> Result<Json<serde_json::Value>, String> {
    let source = normalize_path(&input.source);
    let dest = normalize_path(&input.destination);

    if !source.exists() {
        return Err("Source not found".to_string());
    }

    // If dest is an existing directory, move source inside it
    let target = if dest.is_dir() {
        dest.join(source.file_name().unwrap_or_default())
    } else {
        dest
    };

    // Create parent dirs if needed
    if let Some(parent) = target.parent() {
        async_fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create parent directories: {}", e))?;
    }

    async_fs::rename(&source, &target)
        .await
        .map_err(|e| format!("Failed to move: {}", e))?;

    Ok(Json(serde_json::json!({ "ok": true, "path": target.to_string_lossy() })))
}

async fn cp_handler(
    Json(input): Json<CpRequest>,
) -> Result<Json<serde_json::Value>, String> {
    let source = normalize_path(&input.source);
    let dest = normalize_path(&input.destination);

    if !source.exists() {
        return Err("Source not found".to_string());
    }

    let recursive = input.recursive.unwrap_or(false);

    if source.is_dir() {
        if !recursive {
            return Err("Source is a directory (use recursive: true)".to_string());
        }

        // If dest exists and is a dir, copy inside it
        let target = if dest.is_dir() {
            dest.join(source.file_name().unwrap_or_default())
        } else {
            dest
        };

        copy_dir_recursive(&source, &target).await?;
        Ok(Json(serde_json::json!({ "ok": true, "path": target.to_string_lossy() })))
    } else {
        let target = if dest.is_dir() {
            dest.join(source.file_name().unwrap_or_default())
        } else {
            if let Some(parent) = dest.parent() {
                async_fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            dest
        };

        async_fs::copy(&source, &target)
            .await
            .map_err(|e| format!("Failed to copy: {}", e))?;
        Ok(Json(serde_json::json!({ "ok": true, "path": target.to_string_lossy() })))
    }
}

async fn copy_dir_recursive(src: &FsPath, dst: &FsPath) -> Result<(), String> {
    async_fs::create_dir_all(dst)
        .await
        .map_err(|e| format!("Failed to create dir: {}", e))?;

    let mut entries = async_fs::read_dir(src)
        .await
        .map_err(|e| format!("Failed to read dir: {}", e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read entry: {}", e))?
    {
        let dest_path = dst.join(entry.file_name());
        let meta = entry
            .metadata()
            .await
            .map_err(|e| format!("Failed to read metadata: {}", e))?;

        if meta.is_dir() {
            Box::pin(copy_dir_recursive(&entry.path(), &dest_path)).await?;
        } else {
            async_fs::copy(entry.path(), &dest_path)
                .await
                .map_err(|e| format!("Failed to copy file: {}", e))?;
        }
    }

    Ok(())
}

async fn stat_handler(
    Json(input): Json<StatRequest>,
) -> Result<Json<StatResult>, String> {
    let path = normalize_path(&input.path);
    if !path.exists() {
        return Err("Path not found".to_string());
    }

    let metadata = async_fs::metadata(&path)
        .await
        .map_err(|e| format!("Failed to read metadata: {}", e))?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| {
            let datetime: chrono::DateTime<chrono::Utc> = t.into();
            Some(datetime.to_rfc3339())
        });
    let permissions = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Some(format!("{:o}", metadata.permissions().mode() & 0o777))
        }
        #[cfg(not(unix))]
        {
            None
        }
    };

    let children = if metadata.is_dir() {
        let mut count = 0usize;
        let mut dir_entries = async_fs::read_dir(&path)
            .await
            .map_err(|e| format!("Failed to read dir: {}", e))?;
        while dir_entries
            .next_entry()
            .await
            .map_err(|e| format!("Failed to read dir: {}", e))?
            .is_some()
        {
            count += 1;
        }
        Some(count)
    } else {
        None
    };

    Ok(Json(StatResult {
        name,
        path: path.to_string_lossy().to_string(),
        is_dir: metadata.is_dir(),
        size: metadata.len(),
        modified,
        permissions,
        extension: ext,
        children,
    }))
}

async fn search_handler(
    Json(input): Json<SearchRequest>,
) -> Result<Json<Vec<FileEntry>>, String> {
    let root = normalize_path(&input.path);
    if !root.exists() {
        return Err("Path not found".to_string());
    }

    let query = input.query.to_lowercase();
    let max_depth = input.max_depth.unwrap_or(20);
    let mut results = Vec::new();

    let walker = walkdir::WalkDir::new(&root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok());

    for entry in walker {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.contains(&query) {
            if let Some(fe) = file_entry_from_path(entry.path()) {
                results.push(fe);
                if results.len() >= 500 {
                    break;
                }
            }
        }
    }

    Ok(Json(results))
}

async fn download_handler(
    Json(input): Json<ReadRequest>,
) -> Result<impl IntoResponse, String> {
    let full_path = normalize_path(&input.path);
    if !full_path.exists() {
        return Err("File not found".to_string());
    }
    if full_path.is_dir() {
        return Err("Cannot download a directory".to_string());
    }

    let mime = mime_guess::from_path(&full_path)
        .first_or_octet_stream()
        .to_string();

    let name = full_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let content = async_fs::read(&full_path)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;

    let headers = [
        (header::CONTENT_TYPE, mime),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", name),
        ),
    ];

    Ok((headers, content))
}

async fn upload_handler(
    State(_state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, String> {
    let mut target_dir = String::new();
    let mut file_name = String::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("Multipart error: {}", e))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "path" => {
                target_dir = field
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read path field: {}", e))?;
            }
            "file" => {
                file_name = field
                    .file_name()
                    .unwrap_or("uploaded_file")
                    .to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| format!("Failed to read file data: {}", e))?;

                let dir = normalize_path(&target_dir);
                async_fs::create_dir_all(&dir)
                    .await
                    .map_err(|e| format!("Failed to create dir: {}", e))?;

                let dest = dir.join(&file_name);
                let mut f = async_fs::File::create(&dest)
                    .await
                    .map_err(|e| format!("Failed to create file: {}", e))?;
                f.write_all(&data)
                    .await
                    .map_err(|e| format!("Failed to write file: {}", e))?;
            }
            _ => {}
        }
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "name": file_name
    })))
}

// ── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let addr: SocketAddr = std::env::var("FILE_MANAGER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:0".to_string())
        .parse()
        .expect("invalid FILE_MANAGER_ADDR");

    let state = Arc::new(AppState {});

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
        ])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/ls", post(ls_handler))
        .route("/read", post(read_handler))
        .route("/write", post(write_handler))
        .route("/mkdir", post(mkdir_handler))
        .route("/rm", post(rm_handler))
        .route("/mv", post(mv_handler))
        .route("/cp", post(cp_handler))
        .route("/stat", post(stat_handler))
        .route("/search", post(search_handler))
        .route("/download", post(download_handler))
        .route("/upload", post(upload_handler))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    println!("FILE_MANAGER_PORT={}", local_addr.port());

    tracing::info!("file-manager listening on {}", local_addr);
    axum::serve(listener, app).await.unwrap();
}
