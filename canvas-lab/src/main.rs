use axum::{
    extract::{DefaultBodyLimit, State},
    http::{Method, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};

struct AppState {
    path: PathBuf,
    document: Mutex<Value>,
}

fn document_path() -> PathBuf {
    let base = dirs_next::data_local_dir()
        .or_else(dirs_next::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("noide").join("canvas-lab.json")
}

async fn load_document(path: &PathBuf) -> Value {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "nodes": [], "snapshots": [] })),
        Err(_) => json!({ "nodes": [], "snapshots": [] }),
    }
}

async fn get_document(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(state.document.lock().await.clone())
}

async fn put_document(
    State(state): State<Arc<AppState>>,
    Json(document): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !document.get("nodes").is_some_and(Value::is_array)
        || !document.get("snapshots").is_some_and(Value::is_array)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Canvas Lab document must contain nodes and snapshots arrays".to_string(),
        ));
    }
    let bytes = serde_json::to_vec(&document)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(parent) = state.path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to create Canvas Lab data directory: {e}"),
            )
        })?;
    }
    tokio::fs::write(&state.path, bytes).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save Canvas Lab document: {e}"),
        )
    })?;
    *state.document.lock().await = document.clone();
    Ok(Json(json!({ "saved": true })))
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("canvas-lab {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let addr: SocketAddr = std::env::var("CANVAS_LAB_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:0".to_string())
        .parse()
        .expect("invalid CANVAS_LAB_ADDR");
    let path = document_path();
    let state = Arc::new(AppState {
        document: Mutex::new(load_document(&path).await),
        path,
    });
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::PUT])
        .allow_headers(Any);
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/document", get(get_document).put(put_document))
        .with_state(state)
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(cors);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind Canvas Lab listener");
    let local_addr = listener.local_addr().expect("failed to read local address");
    println!("CANVAS_LAB_PORT={}", local_addr.port());
    axum::serve(listener, app)
        .await
        .expect("Canvas Lab server failed");
}
