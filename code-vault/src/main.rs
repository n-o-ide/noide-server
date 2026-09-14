use axum::{
    extract::{Path, State},
    http::Method,
    response::IntoResponse,
    routing::{get, post},
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
struct Snippet {
    id: i64,
    title: String,
    language: String,
    content: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct CreateSnippet {
    title: String,
    language: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ImportSnippets {
    snippets: Vec<CreateSnippet>,
}

#[derive(Debug, Deserialize)]
struct UpdateSnippet {
    title: Option<String>,
    language: Option<String>,
    content: Option<String>,
}

struct AppState {
    db: Mutex<Connection>,
}

// ── Database ───────────────────────────────────────────────────────────────

fn db_path() -> std::path::PathBuf {
    let base = dirs_next::data_local_dir()
        .or_else(|| dirs_next::data_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("noide").join("code-vault.db")
}

fn open_db() -> Connection {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&path).expect("failed to open code-vault database");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snippets (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            title       TEXT NOT NULL DEFAULT '',
            language    TEXT NOT NULL DEFAULT 'plaintext',
            content     TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .expect("failed to create snippets table");
    conn
}

// ── Handlers ───────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true }))
}

async fn list_snippets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare("SELECT id, title, language, content, created_at, updated_at FROM snippets ORDER BY updated_at DESC")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(Snippet {
                id: row.get(0)?,
                title: row.get(1)?,
                language: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .unwrap();
    let snippets: Vec<Snippet> = rows.filter_map(|r| r.ok()).collect();
    Json(snippets)
}

async fn get_snippet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, String> {
    let db = state.db.lock().await;
    let snippet = db
        .query_row(
            "SELECT id, title, language, content, created_at, updated_at FROM snippets WHERE id = ?1",
            params![id],
            |row| {
                Ok(Snippet {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    language: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(snippet))
}

async fn create_snippet(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateSnippet>,
) -> Result<impl IntoResponse, String> {
    let db = state.db.lock().await;
    db.execute(
        "INSERT INTO snippets (title, language, content) VALUES (?1, ?2, ?3)",
        params![input.title, input.language, input.content],
    )
    .map_err(|e| e.to_string())?;
    let id = db.last_insert_rowid();
    let snippet = db
        .query_row(
            "SELECT id, title, language, content, created_at, updated_at FROM snippets WHERE id = ?1",
            params![id],
            |row| {
                Ok(Snippet {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    language: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(snippet))
}

async fn update_snippet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateSnippet>,
) -> Result<impl IntoResponse, String> {
    let db = state.db.lock().await;
    if let Some(ref title) = input.title {
        db.execute(
            "UPDATE snippets SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![title, id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(ref language) = input.language {
        db.execute(
            "UPDATE snippets SET language = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![language, id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(ref content) = input.content {
        db.execute(
            "UPDATE snippets SET content = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![content, id],
        )
        .map_err(|e| e.to_string())?;
    }
    let snippet = db
        .query_row(
            "SELECT id, title, language, content, created_at, updated_at FROM snippets WHERE id = ?1",
            params![id],
            |row| {
                Ok(Snippet {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    language: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Json(snippet))
}

async fn delete_snippet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, String> {
    let db = state.db.lock().await;
    let affected = db
        .execute("DELETE FROM snippets WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(Json(serde_json::json!({ "deleted": affected > 0 })))
}

async fn export_db() -> Result<impl IntoResponse, String> {
    let path = db_path();
    let data = std::fs::read(&path).map_err(|e| format!("failed to read database: {}", e))?;
    use axum::body::Body;
    use axum::http::{header, Response};
    Ok(Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/x-sqlite3")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"code-vault.db\"",
        )
        .body(Body::from(data))
        .unwrap())
}

async fn export_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    let mut stmt = db
        .prepare("SELECT id, title, language, content, created_at, updated_at FROM snippets ORDER BY updated_at DESC")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(Snippet {
                id: row.get(0)?,
                title: row.get(1)?,
                language: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .unwrap();
    let snippets: Vec<Snippet> = rows.filter_map(|r| r.ok()).collect();
    Json(snippets)
}

async fn import_json(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ImportSnippets>,
) -> Result<Json<serde_json::Value>, String> {
    let db = state.db.lock().await;
    let mut imported = 0;
    for s in &input.snippets {
        db.execute(
            "INSERT INTO snippets (title, language, content) VALUES (?1, ?2, ?3)",
            params![s.title, s.language, s.content],
        )
        .map_err(|e| e.to_string())?;
        imported += 1;
    }
    Ok(Json(serde_json::json!({ "imported": imported })))
}

// ── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let addr: SocketAddr = std::env::var("CODE_VAULT_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:0".to_string())
        .parse()
        .expect("invalid CODE_VAULT_ADDR");

    let state = Arc::new(AppState {
        db: Mutex::new(open_db()),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/snippets", get(list_snippets).post(create_snippet))
        .route(
            "/snippets/:id",
            get(get_snippet).put(update_snippet).delete(delete_snippet),
        )
        .route("/export", get(export_db))
        .route("/export/json", get(export_json))
        .route("/import", post(import_json))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    // Print the port so noide-server can discover it.
    println!("CODE_VAULT_PORT={}", local_addr.port());

    tracing::info!("code-vault listening on {}", local_addr);
    axum::serve(listener, app).await.unwrap();
}
