//! HTTP API server for pdf-inspector.
//!
//! ```text
//! pdf-api                          # listen on 0.0.0.0:8080
//! pdf-api --bind 127.0.0.1:9000    # override the listen address
//! pdf-api --help
//! ```
//!
//! Configuration comes from the environment; flags are a convenience for local
//! runs and take precedence:
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `PDF_INSPECTOR_BIND` | `0.0.0.0:8080` | listen address |
//! | `PDF_INSPECTOR_PORT` / `PORT` | – | overrides the port only |
//! | `PDF_INSPECTOR_API_KEY` | unset | when set, `/v1/*` requires it |
//! | `PDF_INSPECTOR_MAX_UPLOAD_MB` | `64` | upload size cap |
//! | `PDF_INSPECTOR_MAX_CONCURRENCY` | CPU count | PDFs parsed at once |
//! | `PDF_INSPECTOR_BCMAPS_DIR` | crate dir | bundled CJK CMaps |
//! | `RUST_LOG` | `info` | log filter |

use pdf_inspector::http::{serve, ServerConfig};
use std::process::ExitCode;

const HELP: &str = "\
pdf-api — HTTP API for pdf-inspector

USAGE:
    pdf-api [OPTIONS]

OPTIONS:
    --bind <ADDR>        Listen address (default 0.0.0.0:8080)
    --port <PORT>        Listen port, keeps the configured host
    --max-upload-mb <N>  Reject bodies larger than N MiB (default 64)
    --concurrency <N>    PDFs parsed at once (default: CPU count)
    -h, --help           Print this help
    -V, --version        Print the version

ENVIRONMENT:
    PDF_INSPECTOR_BIND, PDF_INSPECTOR_PORT, PDF_INSPECTOR_API_KEY,
    PDF_INSPECTOR_MAX_UPLOAD_MB, PDF_INSPECTOR_MAX_CONCURRENCY,
    PDF_INSPECTOR_BCMAPS_DIR, RUST_LOG

Endpoint directory: GET /   ·   OpenAPI: GET /openapi.json
";

fn main() -> ExitCode {
    // Default to `info` so a container without RUST_LOG still logs requests.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = match parse_args() {
        Ok(Some(config)) => config,
        // --help / --version already printed.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("\n{HELP}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(serve(config)) {
        Ok(()) => {
            log::info!("shutdown complete");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: server failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Returns `Ok(None)` when the process should exit successfully without
/// serving (`--help`, `--version`).
fn parse_args() -> Result<Option<ServerConfig>, String> {
    let mut config = ServerConfig::from_env()?;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("pdf-api {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--bind" => {
                config.bind = next_value(&mut args, "--bind")?;
            }
            "--port" => {
                let port = next_value(&mut args, "--port")?;
                port.parse::<u16>()
                    .map_err(|_| format!("--port: `{port}` is not a valid port"))?;
                let host = config
                    .bind
                    .rsplit_once(':')
                    .map(|(host, _)| host.to_string())
                    .unwrap_or_else(|| "0.0.0.0".to_string());
                config.bind = format!("{host}:{port}");
            }
            "--max-upload-mb" => {
                let value = next_value(&mut args, "--max-upload-mb")?;
                let mb: usize = value
                    .parse()
                    .map_err(|_| format!("--max-upload-mb: `{value}` is not a number"))?;
                if mb == 0 {
                    return Err("--max-upload-mb must be greater than 0".to_string());
                }
                config.max_upload_bytes = mb * 1024 * 1024;
            }
            "--concurrency" => {
                let value = next_value(&mut args, "--concurrency")?;
                let workers: usize = value
                    .parse()
                    .map_err(|_| format!("--concurrency: `{value}` is not a number"))?;
                if workers == 0 {
                    return Err("--concurrency must be greater than 0".to_string());
                }
                config.max_concurrency = workers;
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    Ok(Some(config))
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}
