use axum::{
    http::{header, Method},
    response::{Html, IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod tunnel;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Standalone port-forward tunnel client + web UI", long_about = None)]
struct Args {
    /// Serve the web UI on this address (e.g. 127.0.0.1:7420)
    #[arg(long, value_name = "ADDR")]
    web: Option<String>,

    /// Tunnel provider: trycloudflare, localhost.run, or localtunnel
    #[arg(long)]
    provider: Option<String>,

    /// Local port to forward
    #[arg(long)]
    port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ForwardRequest {
    provider: String,
    port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ForwardResponse {
    ok: bool,
    message: String,
}

#[derive(Clone)]
struct ActiveTunnel {
    child: Arc<Mutex<Option<tokio::process::Child>>>,
}

#[derive(Clone)]
struct AppState {
    active: Arc<Mutex<Option<(String, u16, ActiveTunnel)>>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let args = Args::parse();

    if let Some(web_addr) = args.web {
        let state = AppState { active: Arc::new(Mutex::new(None)) };
        let app = Router::new()
            .route("/", get(serve_ui))
            .route("/api/forward", get(sse_forward).post(json_forward))
            .route("/api/stop", post(stop_forward))
            .route("/api/install", post(install_tool))
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(
                        CorsLayer::new()
                            .allow_origin(Any)
                            .allow_methods([Method::GET, Method::POST])
                            .allow_headers([header::CONTENT_TYPE]),
                    ),
            )
            .with_state(state);

        let addr: SocketAddr = web_addr.parse().expect("Invalid --web address");
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[PORT-FORWARD] Failed to bind {}: {}", addr, e);
                std::process::exit(1);
            }
        };
        let actual = listener.local_addr().unwrap();
        eprintln!("[PORT-FORWARD] UI listening on http://{}", actual);
        axum::serve(listener, app).await.unwrap();
        return;
    }

    // CLI mode
    let provider = args.provider.unwrap_or_default();
    let port = args.port.unwrap_or(3000);
    if provider.is_empty() {
        eprintln!("Usage: port-forward --web 127.0.0.1:7420");
        eprintln!("   or: port-forward --provider <name> --port <port>");
        std::process::exit(1);
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            eprintln!("{}", line);
        }
    });
    match tunnel::run_tunnel(&provider, port, tx).await {
        Ok(Some(url)) => {
            use std::io::Write;
            println!("[URL] {}", url);
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
        Ok(None) => {
            eprintln!("[ERROR] Tunnel ended without producing a URL");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("[ERROR] {}", e);
            std::process::exit(1);
        }
    }
}

async fn serve_ui() -> Response {
    Html(include_str!("ui.html")).into_response()
}

async fn sse_forward(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let port = params.get("port").and_then(|p| p.parse().ok()).unwrap_or(3000);

    stop_tunnel_internal(&state).await;

    let (tx, rx) = tokio::sync::mpsc::channel(128);
    let tx_for_stream = tx.clone();

    let state_clone = state.clone();
    let provider_clone = provider.clone();
    let tx_for_tunnel = tx_for_stream.clone();

    tokio::spawn(async move {
        let result = tunnel::run_tunnel(&provider_clone, port, tx_for_tunnel).await;

        if let Err(e) = result {
            let _ = tx_for_stream
                .blocking_send(format!("event: error\ndata: {}\n\n", escape_json(&e)));
        }

        let mut active = state_clone.active.lock().await;
        if active.as_ref().map(|(p, pr, _)| p == &provider_clone && *pr == port).unwrap_or(false) {
            *active = None;
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(|msg| {
        Ok::<_, Infallible>(axum::response::sse::Event::default().data(msg))
    });

    Sse::new(stream).into_response()
}

async fn json_forward(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(req): Json<ForwardRequest>,
) -> Response {
    stop_tunnel_internal(&state).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let state_clone = state.clone();
    let provider = req.provider.clone();
    let port = req.port;
    let child_holder: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));

    let child_for_spawn = child_holder.clone();
    let tx_for_tunnel = tx.clone();
    tokio::spawn(async move {
        let result = tunnel::run_tunnel_owned(&provider, port, tx_for_tunnel, child_for_spawn).await;

        if let Err(e) = result {
            tracing::error!("tunnel error: {}", e);
        }

        let mut active = state_clone.active.lock().await;
        if active.as_ref().map(|(p, pr, _)| p == &provider && *pr == port).unwrap_or(false) {
            *active = None;
        }
    });

    let mut logs = Vec::new();
    let mut url = None;
    while let Some(line) = rx.recv().await {
        logs.push(line.clone());
        if let Some(u) = tunnel::parse_url_any(&line) {
            url = Some(u);
            break;
        }
        if line.contains("ERROR") || line.to_lowercase().contains("not installed") {
            break;
        }
    }

    match url {
        Some(u) => Json(ForwardResponse { ok: true, message: u }).into_response(),
        None => Json(ForwardResponse { ok: false, message: logs.join("\n") }).into_response(),
    }
}

async fn stop_forward(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
    stop_tunnel_internal(&state).await;
    Json(ForwardResponse { ok: true, message: "stopped".into() }).into_response()
}

async fn install_tool(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let provider = params.get("provider").cloned().unwrap_or_default();
    if provider.is_empty() {
        return Json(ForwardResponse { ok: false, message: "missing provider".into() }).into_response();
    }

    match tunnel::install_provider(&provider).await {
        Ok(output) => Json(ForwardResponse { ok: true, message: output }).into_response(),
        Err(e) => Json(ForwardResponse { ok: false, message: e }).into_response(),
    }
}

async fn stop_tunnel_internal(state: &AppState) {
    let mut active = state.active.lock().await;
    if let Some((_, _, tunnel)) = active.take() {
        if let Some(mut child_opt) = tunnel.child.lock().await.take() {
            let _ = child_opt.start_kill();
            let _ = child_opt.wait().await;
        }
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}
