use axum::{
    extract::{Path, State},
    http::Method,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Folder {
    id: i64,
    name: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateFolder {
    name: String,
}

#[derive(Debug, Deserialize)]
struct UpdateFolder {
    name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedRequest {
    id: i64,
    name: String,
    method: String,
    url: String,
    headers: String,
    body: String,
    folder_id: Option<i64>,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    name: String,
    method: String,
    url: String,
    headers: String,
    body: String,
    folder_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    method: Option<String>,
    url: Option<String>,
    headers: Option<String>,
    body: Option<String>,
    folder_id: Option<Option<i64>>,
}

#[derive(Debug, Deserialize)]
struct SendRequest {
    method: String,
    url: String,
    headers: Option<Vec<HeaderPair>>,
    body: Option<String>,
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HeaderPair {
    key: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct HttpResponse {
    status: u16,
    status_text: String,
    headers: Vec<HeaderPair>,
    body: String,
    elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ImportOpenApi {
    /// The OpenAPI spec as JSON value
    spec: serde_json::Value,
    /// Optional base URL to prepend to paths
    base_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct CollectionItem {
    id: i64,
    name: String,
    created_at: String,
    requests: Vec<SavedRequest>,
}

struct AppState {
    db: Mutex<Connection>,
    client: reqwest::Client,
}

// ── Database ───────────────────────────────────────────────────────────────

fn db_path() -> std::path::PathBuf {
    let base = dirs_next::data_local_dir()
        .or_else(|| dirs_next::data_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("noide").join("http-request.db")
}

fn open_db() -> Connection {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&path).expect("failed to open http-request database");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS folders (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS requests (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL DEFAULT '',
            method      TEXT NOT NULL DEFAULT 'GET',
            url         TEXT NOT NULL DEFAULT '',
            headers     TEXT NOT NULL DEFAULT '[]',
            body        TEXT NOT NULL DEFAULT '',
            folder_id   INTEGER,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (folder_id) REFERENCES folders(id) ON DELETE SET NULL
        );"
    ).expect("failed to create tables");

    // Add folder_id column if missing (migration for existing DBs)
    let has_folder_id: bool = conn
        .prepare("PRAGMA table_info(requests)")
        .ok()
        .and_then(|mut stmt| {
            let cols: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
            Some(cols.iter().any(|col| col == "folder_id"))
        })
        .unwrap_or(false);
    if !has_folder_id {
        conn.execute_batch("ALTER TABLE requests ADD COLUMN folder_id INTEGER;")
            .expect("failed to add folder_id column");
    }

    conn
}

// ── Handlers ───────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true }))
}

async fn send_http(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SendRequest>,
) -> Result<Json<HttpResponse>, String> {
    let method = input.method.to_uppercase();
    let client = &state.client;

    let mut req = match method.as_str() {
        "POST" => client.post(&input.url),
        "PUT" => client.put(&input.url),
        "DELETE" => client.delete(&input.url),
        "PATCH" => client.patch(&input.url),
        "HEAD" => client.head(&input.url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, &input.url),
        _ => client.get(&input.url),
    };

    if let Some(headers) = &input.headers {
        for h in headers {
            if !h.key.is_empty() {
                req = req.header(&h.key, &h.value);
            }
        }
    }

    if let Some(body) = &input.body {
        if !body.is_empty() {
            req = req.body(body.clone());
        }
    }

    let timeout = std::time::Duration::from_secs(input.timeout.unwrap_or(30));
    req = req.timeout(timeout);

    let start = std::time::Instant::now();
    let resp = req.send().await.map_err(|e| format!("Request failed: {}", e))?;
    let elapsed = start.elapsed().as_millis() as u64;

    let status = resp.status().as_u16();
    let status_text = resp.status().canonical_reason().unwrap_or("Unknown").to_string();

    let resp_headers: Vec<HeaderPair> = resp.headers()
        .iter()
        .map(|(k, v)| HeaderPair {
            key: k.to_string(),
            value: v.to_str().unwrap_or("").to_string(),
        })
        .collect();

    let body = resp.text().await.map_err(|e| format!("Failed to read response: {}", e))?;

    Ok(Json(HttpResponse {
        status,
        status_text,
        headers: resp_headers,
        body,
        elapsed_ms: elapsed,
    }))
}

// ── Folder handlers ──

async fn list_folders(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare("SELECT id, name, created_at FROM folders ORDER BY created_at ASC")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(Folder {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .unwrap();
    let folders: Vec<Folder> = rows.filter_map(|r| r.ok()).collect();
    Json(folders)
}

async fn create_folder(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateFolder>,
) -> Result<Json<Folder>, String> {
    let db = state.db.lock().await;
    db.execute(
        "INSERT INTO folders (name) VALUES (?1)",
        params![input.name],
    )
    .map_err(|e| e.to_string())?;
    let id = db.last_insert_rowid();
    let folder = db
        .query_row(
            "SELECT id, name, created_at FROM folders WHERE id = ?1",
            params![id],
            |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(folder))
}

async fn update_folder(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateFolder>,
) -> Result<Json<Folder>, String> {
    let db = state.db.lock().await;
    if let Some(ref name) = input.name {
        db.execute("UPDATE folders SET name = ?1 WHERE id = ?2", params![name, id])
            .map_err(|e| e.to_string())?;
    }
    let folder = db
        .query_row(
            "SELECT id, name, created_at FROM folders WHERE id = ?1",
            params![id],
            |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(folder))
}

async fn delete_folder(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, String> {
    let db = state.db.lock().await;
    db.execute("UPDATE requests SET folder_id = NULL WHERE folder_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    let affected = db
        .execute("DELETE FROM folders WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(Json(serde_json::json!({ "deleted": affected > 0 })))
}

// ── Request handlers ──

async fn list_requests(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;

    // Get all folders
    let mut folder_stmt = db
        .prepare("SELECT id, name, created_at FROM folders ORDER BY created_at ASC")
        .unwrap();
    let folders: Vec<Folder> = folder_stmt
        .query_map([], |row| {
            Ok(Folder {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // Get all requests
    let mut req_stmt = db
        .prepare("SELECT id, name, method, url, headers, body, folder_id, created_at FROM requests ORDER BY created_at DESC")
        .unwrap();
    let requests: Vec<SavedRequest> = req_stmt
        .query_map([], |row| {
            Ok(SavedRequest {
                id: row.get(0)?,
                name: row.get(1)?,
                method: row.get(2)?,
                url: row.get(3)?,
                headers: row.get(4)?,
                body: row.get(5)?,
                folder_id: row.get(6)?,
                created_at: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // Build grouped collection
    let mut result: Vec<CollectionItem> = folders
        .iter()
        .map(|f| CollectionItem {
            id: f.id,
            name: f.name.clone(),
            created_at: f.created_at.clone(),
            requests: vec![],
        })
        .collect();

    for req in &requests {
        match req.folder_id {
            Some(fid) => {
                if let Some(item) = result.iter_mut().find(|c| c.id == fid) {
                    item.requests.push(req.clone());
                }
            }
            None => {
                // Requests without a folder go into a special "ungrouped" entry
                match result.iter_mut().find(|c| c.id == 0) {
                    Some(item) => item.requests.push(req.clone()),
                    None => result.push(CollectionItem {
                        id: 0,
                        name: String::new(),
                        created_at: String::new(),
                        requests: vec![req.clone()],
                    }),
                }
            }
        }
    }

    // Remove empty folders
    result.retain(|c| !c.requests.is_empty() || c.id != 0);

    Json(result)
}

async fn get_request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<SavedRequest>, String> {
    let db = state.db.lock().await;
    let req = db
        .query_row(
            "SELECT id, name, method, url, headers, body, folder_id, created_at FROM requests WHERE id = ?1",
            params![id],
            |row| {
                Ok(SavedRequest {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    method: row.get(2)?,
                    url: row.get(3)?,
                    headers: row.get(4)?,
                    body: row.get(5)?,
                    folder_id: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(req))
}

async fn create_request(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateRequest>,
) -> Result<Json<SavedRequest>, String> {
    let db = state.db.lock().await;
    db.execute(
        "INSERT INTO requests (name, method, url, headers, body, folder_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![input.name, input.method, input.url, input.headers, input.body, input.folder_id],
    )
    .map_err(|e| e.to_string())?;
    let id = db.last_insert_rowid();
    let req = db
        .query_row(
            "SELECT id, name, method, url, headers, body, folder_id, created_at FROM requests WHERE id = ?1",
            params![id],
            |row| {
                Ok(SavedRequest {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    method: row.get(2)?,
                    url: row.get(3)?,
                    headers: row.get(4)?,
                    body: row.get(5)?,
                    folder_id: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(req))
}

async fn update_request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateRequest>,
) -> Result<Json<SavedRequest>, String> {
    let db = state.db.lock().await;
    if let Some(ref name) = input.name {
        db.execute("UPDATE requests SET name = ?1 WHERE id = ?2", params![name, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref method) = input.method {
        db.execute("UPDATE requests SET method = ?1 WHERE id = ?2", params![method, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref url) = input.url {
        db.execute("UPDATE requests SET url = ?1 WHERE id = ?2", params![url, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref headers) = input.headers {
        db.execute("UPDATE requests SET headers = ?1 WHERE id = ?2", params![headers, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref body) = input.body {
        db.execute("UPDATE requests SET body = ?1 WHERE id = ?2", params![body, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref folder_id) = input.folder_id {
        db.execute("UPDATE requests SET folder_id = ?1 WHERE id = ?2", params![folder_id, id])
            .map_err(|e| e.to_string())?;
    }
    let req = db
        .query_row(
            "SELECT id, name, method, url, headers, body, folder_id, created_at FROM requests WHERE id = ?1",
            params![id],
            |row| {
                Ok(SavedRequest {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    method: row.get(2)?,
                    url: row.get(3)?,
                    headers: row.get(4)?,
                    body: row.get(5)?,
                    folder_id: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(req))
}

async fn delete_request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, String> {
    let db = state.db.lock().await;
    let affected = db
        .execute("DELETE FROM requests WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(Json(serde_json::json!({ "deleted": affected > 0 })))
}

// ── OpenAPI Import ──

async fn import_openapi(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ImportOpenApi>,
) -> Result<Json<serde_json::Value>, String> {
    let spec = &input.spec;
    let base_url = input.base_url.as_deref().unwrap_or("");

    // Extract servers for base URL if not provided
    let resolved_base = if base_url.is_empty() {
        spec.get("servers")
            .and_then(|s| s.as_array())
            .and_then(|arr| arr.first())
            .and_then(|s| s.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        base_url.to_string()
    };

    let paths = spec.get("paths")
        .and_then(|p| p.as_object())
        .ok_or_else(|| "No paths found in OpenAPI spec".to_string())?;

    let db = state.db.lock().await;
    let mut imported_folders = 0;
    let mut imported_requests = 0;

    for (path, path_item) in paths {
        let path_obj = path_item.as_object().ok_or("Path item is not an object")?;

        // Use tags to group into folders; first tag is used
        let tag = path_item.get("tags")
            .and_then(|t| t.as_array())
            .and_then(|arr| arr.first())
            .and_then(|t| t.as_str())
            .unwrap_or("Imported");

        // Ensure folder exists
        let folder_id: i64 = {
            let existing: Option<i64> = db
                .query_row(
                    "SELECT id FROM folders WHERE name = ?1",
                    params![tag],
                    |row| row.get(0),
                )
                .ok();
            match existing {
                Some(id) => id,
                None => {
                    db.execute(
                        "INSERT INTO folders (name) VALUES (?1)",
                        params![tag],
                    )
                    .map_err(|e| e.to_string())?;
                    let id = db.last_insert_rowid();
                    imported_folders += 1;
                    id
                }
            }
        };

        let methods = ["get", "post", "put", "delete", "patch", "head", "options"];
        for method_name in &methods {
            if let Some(operation) = path_obj.get(*method_name) {
                let operation_id = operation.get("operationId")
                    .and_then(|o| o.as_str())
                    .unwrap_or("");
                let summary = operation.get("summary")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");

                let name = if !operation_id.is_empty() {
                    operation_id.to_string()
                } else if !summary.is_empty() {
                    summary.to_string()
                } else {
                    format!("{} {}", method_name.to_uppercase(), path)
                };

                let url = format!("{}{}", resolved_base, path);
                let method_upper = method_name.to_uppercase();

                // Build headers from parameters
                let headers_json = "[]".to_string();

                // Build body from requestBody if present
                let body = if let Some(request_body) = operation.get("requestBody") {
                    if let Some(content) = request_body.get("content") {
                        if let Some(json_content) = content.get("application/json") {
                            if let Some(schema) = json_content.get("schema") {
                                // Generate a sample body from the schema
                                generate_sample_body(schema).unwrap_or_default()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                db.execute(
                    "INSERT INTO requests (name, method, url, headers, body, folder_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![name, method_upper, url, headers_json, body, folder_id],
                )
                .map_err(|e| e.to_string())?;
                imported_requests += 1;
            }
        }
    }

    Ok(Json(serde_json::json!({
        "imported_folders": imported_folders,
        "imported_requests": imported_requests,
    })))
}

/// Generate a simple sample body from a JSON Schema
fn generate_sample_body(schema: &serde_json::Value) -> Option<String> {
    let obj = schema.as_object()?;
    let sample = generate_from_schema(obj)?;
    serde_json::to_string_pretty(&sample).ok()
}

fn generate_from_schema(obj: &serde_json::Map<String, serde_json::Value>) -> Option<serde_json::Value> {
    if let Some(r#type) = obj.get("type").and_then(|t| t.as_str()) {
        return Some(match r#type {
            "object" => {
                let mut map = serde_json::Map::new();
                if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                    for (key, prop_schema) in props {
                        if let Some(prop_obj) = prop_schema.as_object() {
                            if let Some(val) = generate_from_schema(prop_obj) {
                                map.insert(key.clone(), val);
                            }
                        }
                    }
                }
                serde_json::Value::Object(map)
            }
            "array" => {
                if let Some(items) = obj.get("items").and_then(|i| i.as_object()) {
                    if let Some(val) = generate_from_schema(items) {
                        serde_json::Value::Array(vec![val])
                    } else {
                        serde_json::Value::Array(vec![])
                    }
                } else {
                    serde_json::Value::Array(vec![])
                }
            }
            "string" => {
                if let Some(values) = obj.get("enum").and_then(|e| e.as_array()) {
                    values.first().cloned().unwrap_or(serde_json::Value::String("string".into()))
                } else {
                    serde_json::Value::String("string".into())
                }
            }
            "integer" | "number" => serde_json::Value::Number(0.into()),
            "boolean" => serde_json::Value::Bool(false),
            "null" => serde_json::Value::Null,
            _ => serde_json::Value::String("string".into()),
        });
    }

    // Handle allOf/oneOf/anyOf by using the first variant
    for compositor in &["allOf", "oneOf", "anyOf"] {
        if let Some(variants) = obj.get(*compositor).and_then(|v| v.as_array()) {
            if let Some(first) = variants.first().and_then(|v| v.as_object()) {
                return generate_from_schema(first);
            }
        }
    }

    Some(serde_json::Value::String("string".into()))
}

async fn export_collections(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db = state.db.lock().await;

    let mut folder_stmt = db
        .prepare("SELECT id, name, created_at FROM folders ORDER BY created_at ASC")
        .unwrap();
    let folders: Vec<Folder> = folder_stmt
        .query_map([], |row| {
            Ok(Folder {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let mut req_stmt = db
        .prepare("SELECT id, name, method, url, headers, body, folder_id, created_at FROM requests ORDER BY created_at DESC")
        .unwrap();
    let requests: Vec<SavedRequest> = req_stmt
        .query_map([], |row| {
            Ok(SavedRequest {
                id: row.get(0)?,
                name: row.get(1)?,
                method: row.get(2)?,
                url: row.get(3)?,
                headers: row.get(4)?,
                body: row.get(5)?,
                folder_id: row.get(6)?,
                created_at: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    Json(serde_json::json!({
        "folders": folders,
        "requests": requests,
    }))
}

async fn import_collections(
    State(state): State<Arc<AppState>>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, String> {
    let db = state.db.lock().await;
    let mut imported_folders = 0;
    let mut imported_requests = 0;

    // Import folders first, mapping old IDs to new IDs
    let mut id_map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    if let Some(folders) = input.get("folders").and_then(|f| f.as_array()) {
        for folder in folders {
            let name = folder.get("name").and_then(|n| n.as_str()).unwrap_or("");
            db.execute(
                "INSERT INTO folders (name) VALUES (?1)",
                params![name],
            )
            .map_err(|e| e.to_string())?;
            let old_id = folder.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
            let new_id = db.last_insert_rowid();
            id_map.insert(old_id, new_id);
            imported_folders += 1;
        }
    }

    if let Some(requests) = input.get("requests").and_then(|r| r.as_array()) {
        for req in requests {
            let name = req.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("GET");
            let url = req.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let headers = req.get("headers").and_then(|h| h.as_str()).unwrap_or("[]");
            let body = req.get("body").and_then(|b| b.as_str()).unwrap_or("");
            let old_folder_id = req.get("folder_id").and_then(|f| f.as_i64());
            let new_folder_id = old_folder_id.and_then(|fid| id_map.get(&fid).copied());

            db.execute(
                "INSERT INTO requests (name, method, url, headers, body, folder_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![name, method, url, headers, body, new_folder_id],
            )
            .map_err(|e| e.to_string())?;
            imported_requests += 1;
        }
    }

    Ok(Json(serde_json::json!({
        "imported_folders": imported_folders,
        "imported_requests": imported_requests,
    })))
}

// ── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let addr: SocketAddr = std::env::var("HTTP_REQUEST_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:0".to_string())
        .parse()
        .expect("invalid HTTP_REQUEST_ADDR");

    let state = Arc::new(AppState {
        db: Mutex::new(open_db()),
        client: reqwest::Client::new(),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/send", post(send_http))
        .route("/folders", get(list_folders).post(create_folder))
        .route("/folders/:id", put(update_folder).delete(delete_folder))
        .route("/collections", get(list_requests).post(create_request))
        .route("/collections/:id", get(get_request).put(update_request).delete(delete_request))
        .route("/import-openapi", post(import_openapi))
        .route("/export", get(export_collections))
        .route("/import", post(import_collections))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    println!("HTTP_REQUEST_PORT={}", local_addr.port());

    tracing::info!("http-request listening on {}", local_addr);
    axum::serve(listener, app).await.unwrap();
}
