//! HTTP API exposing the full `pdf-inspector` library surface.
//!
//! Enabled by the `server` cargo feature; the binary is `pdf-api`.
//!
//! Design notes:
//!
//! * **Blocking work is isolated.** PDF parsing is CPU-bound and can take
//!   seconds, so every call runs on `spawn_blocking` behind a semaphore that
//!   caps how many documents are parsed at once. Without the cap, a burst of
//!   large uploads would spawn unbounded blocking threads and thrash memory.
//! * **Stateless.** Nothing is written to disk and nothing is cached between
//!   requests, so instances scale horizontally behind any load balancer.
//! * **One body contract for every endpoint** — see [`payload`].

pub mod dto;
pub mod error;
pub mod handlers;
pub mod openapi;
pub mod payload;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub use error::ApiError;

/// Default listen address: every interface, port 8080.
pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default upload cap, in megabytes.
pub const DEFAULT_MAX_UPLOAD_MB: usize = 64;

/// Server configuration, normally built from the environment.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `host:port` to listen on.
    pub bind: String,
    /// When set, every `/v1/*` request must present this key as
    /// `Authorization: Bearer <key>` or `X-API-Key: <key>`.
    pub api_key: Option<String>,
    /// Largest accepted request body, in bytes.
    pub max_upload_bytes: usize,
    /// How many PDFs may be parsed concurrently.
    pub max_concurrency: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            api_key: None,
            max_upload_bytes: DEFAULT_MAX_UPLOAD_MB * 1024 * 1024,
            max_concurrency: default_concurrency(),
        }
    }
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
}

impl ServerConfig {
    /// Read configuration from environment variables.
    ///
    /// | Variable | Default |
    /// |---|---|
    /// | `PDF_INSPECTOR_BIND` | `0.0.0.0:8080` |
    /// | `PDF_INSPECTOR_PORT` | overrides the port of `BIND` |
    /// | `PDF_INSPECTOR_API_KEY` | unset (no auth) |
    /// | `PDF_INSPECTOR_MAX_UPLOAD_MB` | `64` |
    /// | `PDF_INSPECTOR_MAX_CONCURRENCY` | CPU count |
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();

        if let Ok(bind) = std::env::var("PDF_INSPECTOR_BIND") {
            if !bind.trim().is_empty() {
                config.bind = bind.trim().to_string();
            }
        }
        // `PORT` is the convention most PaaS runtimes inject.
        if let Some(port) = env_var("PDF_INSPECTOR_PORT").or_else(|| env_var("PORT")) {
            let port: u16 = port
                .parse()
                .map_err(|_| format!("PDF_INSPECTOR_PORT: `{port}` is not a valid port"))?;
            let host = config
                .bind
                .rsplit_once(':')
                .map(|(host, _)| host.to_string())
                .unwrap_or_else(|| "0.0.0.0".to_string());
            config.bind = format!("{host}:{port}");
        }

        config.api_key = env_var("PDF_INSPECTOR_API_KEY");

        if let Some(mb) = env_var("PDF_INSPECTOR_MAX_UPLOAD_MB") {
            let mb: usize = mb
                .parse()
                .map_err(|_| format!("PDF_INSPECTOR_MAX_UPLOAD_MB: `{mb}` is not a number"))?;
            if mb == 0 {
                return Err("PDF_INSPECTOR_MAX_UPLOAD_MB must be greater than 0".to_string());
            }
            config.max_upload_bytes = mb * 1024 * 1024;
        }

        if let Some(value) = env_var("PDF_INSPECTOR_MAX_CONCURRENCY") {
            let parsed: usize = value
                .parse()
                .map_err(|_| format!("PDF_INSPECTOR_MAX_CONCURRENCY: `{value}` is not a number"))?;
            if parsed == 0 {
                return Err("PDF_INSPECTOR_MAX_CONCURRENCY must be greater than 0".to_string());
            }
            config.max_concurrency = parsed;
        }

        Ok(config)
    }
}

/// Read an environment variable, treating blank as unset.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

/// Shared, cheaply-cloneable request state.
#[derive(Clone)]
pub struct AppState {
    pub max_upload_bytes: usize,
    api_key: Option<Arc<String>>,
    workers: Arc<Semaphore>,
}

impl AppState {
    pub fn new(config: &ServerConfig) -> Self {
        Self {
            max_upload_bytes: config.max_upload_bytes,
            api_key: config.api_key.clone().map(Arc::new),
            workers: Arc::new(Semaphore::new(config.max_concurrency)),
        }
    }

    /// Run CPU-bound library work on a blocking thread, bounded by the worker
    /// semaphore so concurrent uploads cannot exhaust memory.
    pub async fn run_blocking<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        F: FnOnce() -> Result<T, crate::PdfError> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ApiError::internal("server is shutting down"))?;

        let outcome = tokio::task::spawn_blocking(move || {
            // Held for the duration of the work, released on return or panic.
            let _permit = permit;
            work()
        })
        .await;

        match outcome {
            Ok(result) => result.map_err(ApiError::from),
            // A panic inside lopdf/the extractor must not take the server down.
            Err(join_error) => Err(ApiError::internal(format!(
                "extraction worker failed: {join_error}"
            ))),
        }
    }
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/process", post(handlers::process))
        // `/markdown` reads better from a shell; same handler.
        .route("/markdown", post(handlers::process))
        .route("/detect", post(handlers::detect))
        .route("/analyze", post(handlers::analyze))
        .route("/classify", post(handlers::classify))
        .route("/text", post(handlers::text))
        .route("/items", post(handlers::items))
        .route("/pages/markdown", post(handlers::pages_markdown))
        .route("/regions/text", post(handlers::regions_text))
        .route("/regions/tables", post(handlers::regions_tables))
        .route("/regions/vector-grid", post(handlers::regions_vector_grid))
        .route("/tables/structure", post(handlers::tables_structure))
        .route(
            "/tables/structure/cells",
            post(handlers::tables_structure_cells),
        )
        .route(
            "/tables/structure/auto",
            post(handlers::tables_structure_auto),
        )
        // `route_layer` runs only for paths this router matched, so an unknown
        // path 404s instead of demanding a key first.
        .route_layer(middleware::from_fn_with_state(state.clone(), require_api_key));

    Router::new()
        .route("/", get(handlers::index))
        .route("/health", get(handlers::health))
        // k8s-style aliases.
        .route("/healthz", get(handlers::health))
        .route("/readyz", get(handlers::health))
        .route("/version", get(handlers::version))
        .route("/openapi.json", get(handlers::openapi))
        .nest("/v1", v1)
        .fallback(handlers::not_found)
        .layer(DefaultBodyLimit::max(state.max_upload_bytes))
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

/// Bind and serve until SIGINT/SIGTERM.
pub async fn serve(config: ServerConfig) -> std::io::Result<()> {
    let state = AppState::new(&config);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    let local_addr = listener.local_addr()?;
    log::info!(
        "pdf-inspector {} listening on http://{local_addr} (max upload {} MiB, {} workers, auth {})",
        env!("CARGO_PKG_VERSION"),
        config.max_upload_bytes / (1024 * 1024),
        config.max_concurrency,
        if config.api_key.is_some() {
            "enabled"
        } else {
            "disabled"
        },
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
}

/// Resolve when the process is asked to stop, so in-flight requests can drain.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            // Cannot install the handler — fall back to Ctrl-C only.
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    log::info!("shutdown signal received, draining in-flight requests");
}

/// Reject `/v1/*` requests without the configured API key.
async fn require_api_key(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(expected) = state.api_key.clone() else {
        return Ok(next.run(request).await);
    };

    // Copy the presented key out before `request` is moved into `next`.
    let presented: Option<String> = {
        let headers = request.headers();
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let value = value.trim();
                value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
            })
            .or_else(|| {
                headers
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok())
            })
            .map(|value| value.trim().to_string())
    };

    match presented {
        Some(key) if secret_eq(key.as_bytes(), expected.as_bytes()) => Ok(next.run(request).await),
        _ => Err(ApiError::unauthorized(
            "missing or invalid API key: send `Authorization: Bearer <key>` or `X-API-Key: <key>`",
        )),
    }
}

/// Length-independent comparison, so a wrong key leaks no byte positions.
fn secret_eq(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// One log line per request: method, path, status, duration.
async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();

    let response = next.run(request).await;

    log::info!(
        "{method} {path} -> {} in {}ms",
        response.status().as_u16(),
        started.elapsed().as_millis()
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_comparison_matches_only_exact_keys() {
        assert!(secret_eq(b"s3cret", b"s3cret"));
        assert!(!secret_eq(b"s3cret", b"s3crEt"));
        assert!(!secret_eq(b"s3cre", b"s3cret"));
        assert!(!secret_eq(b"", b"s3cret"));
    }

    #[test]
    fn default_config_is_usable_without_env() {
        let config = ServerConfig::default();
        assert_eq!(config.bind, DEFAULT_BIND);
        assert!(config.api_key.is_none());
        assert_eq!(config.max_upload_bytes, DEFAULT_MAX_UPLOAD_MB * 1024 * 1024);
        assert!(config.max_concurrency >= 1);
    }
}
