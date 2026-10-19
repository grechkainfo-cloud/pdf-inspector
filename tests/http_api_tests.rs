//! End-to-end tests for the HTTP API.
//!
//! The router is driven directly through `tower::ServiceExt::oneshot`, so no
//! socket is bound and the tests stay deterministic under parallel execution.
//!
//! Run with: `cargo test --features server`
#![cfg(feature = "server")]

use axum::body::Body;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::Router;
use pdf_inspector::http::{router, AppState, ServerConfig};
use serde_json::Value;
use tower::ServiceExt;

const TEXT_PDF: &str = "tests/fixtures/text_page_with_watermark_image.pdf";
const ENCRYPTED_PDF: &str = "tests/fixtures/encrypted-secret123.pdf";

fn app() -> Router {
    router(AppState::new(&ServerConfig::default()))
}

fn app_with(config: ServerConfig) -> Router {
    router(AppState::new(&config))
}

fn pdf_bytes(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
}

fn raw_pdf_request(uri: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/pdf")
        .body(Body::from(bytes))
        .unwrap()
}

fn json_request(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Build a minimal multipart body: one file part plus text option parts.
fn multipart_request(uri: &str, pdf: Vec<u8>, fields: &[(&str, &str)]) -> Request<Body> {
    const BOUNDARY: &str = "pdfinspectorboundary";
    let mut body: Vec<u8> = Vec::new();

    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"doc.pdf\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/pdf\r\n\r\n");
    body.extend_from_slice(&pdf);
    body.extend_from_slice(b"\r\n");

    for (name, value) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn send(app: Router, request: Request<Body>) -> (StatusCode, String, Vec<u8>) {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, content_type, body)
}

async fn send_json(app: Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, _, body) = send(app, request).await;
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|e| panic!("expected JSON body, got {:?}: {e}", String::from_utf8_lossy(&body)));
    (status, value)
}

// -------------------------------------------------------------------------
// Service endpoints
// -------------------------------------------------------------------------

#[tokio::test]
async fn health_reports_ok() {
    let request = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn openapi_document_is_served() {
    let request = Request::builder()
        .uri("/openapi.json")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["paths"]["/v1/process"].is_object());
}

#[tokio::test]
async fn console_is_served_at_ui() {
    let request = Request::builder().uri("/ui").body(Body::empty()).unwrap();
    let (status, content_type, body) = send(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/html"));
    assert!(String::from_utf8_lossy(&body).contains("<!doctype html>"));
}

#[tokio::test]
async fn root_serves_the_console_to_browsers_and_json_to_tools() {
    // A browser sends Accept: text/html — it should land on something usable.
    let request = Request::builder()
        .uri("/")
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .body(Body::empty())
        .unwrap();
    let (status, content_type, _) = send(app(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/html"));

    // curl and health checkers keep the JSON directory.
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let (status, body) = send_json(app(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["service"], "pdf-inspector");
    assert_eq!(body["console"], "/ui");
}

#[tokio::test]
async fn console_can_be_disabled() {
    let config = ServerConfig {
        ui_enabled: false,
        ..ServerConfig::default()
    };

    let request = Request::builder().uri("/ui").body(Body::empty()).unwrap();
    let (status, body) = send_json(app_with(config.clone()), request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "ui_disabled");

    // With the console off, a browser hitting the root still gets the JSON.
    let request = Request::builder()
        .uri("/")
        .header(ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(app_with(config), request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("pdf-inspector"));
}

#[tokio::test]
async fn console_never_requires_an_api_key() {
    // The page is a static client; locking it away would only stop people
    // from discovering how to authenticate.
    let request = Request::builder().uri("/ui").body(Body::empty()).unwrap();
    let (status, _, _) = send(app_with(authed_config()), request).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn unknown_route_returns_a_json_error() {
    let request = Request::builder()
        .uri("/v2/process")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

// -------------------------------------------------------------------------
// Body shapes: raw, JSON, multipart
// -------------------------------------------------------------------------

#[tokio::test]
async fn process_accepts_a_raw_pdf_body() {
    let request = raw_pdf_request("/v1/process", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pdf_type"], "text_based");
    assert!(body["page_count"].as_u64().unwrap() >= 1);
    assert!(!body["markdown"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn process_accepts_base64_json() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let body = serde_json::json!({
        "pdf_base64": STANDARD.encode(pdf_bytes(TEXT_PDF)),
        "compact": true,
    });
    let (status, body) = send_json(app(), json_request("/v1/process", body)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["markdown_length"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn process_accepts_multipart_with_option_parts() {
    let request = multipart_request(
        "/v1/process",
        pdf_bytes(TEXT_PDF),
        &[("compact", "true"), ("include_page_numbers", "true")],
    );
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    let markdown = body["markdown"].as_str().unwrap();
    assert!(markdown.contains("<!-- Page 1 -->"), "got: {markdown}");
}

#[tokio::test]
async fn multipart_options_may_be_one_json_part() {
    let request = multipart_request(
        "/v1/process",
        pdf_bytes(TEXT_PDF),
        &[("options", r#"{"include_page_numbers": true}"#)],
    );
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["markdown"].as_str().unwrap().contains("<!-- Page 1 -->"));
}

// -------------------------------------------------------------------------
// Per-endpoint behaviour
// -------------------------------------------------------------------------

#[tokio::test]
async fn format_markdown_returns_the_raw_document() {
    let request = raw_pdf_request("/v1/process?format=markdown", pdf_bytes(TEXT_PDF));
    let (status, content_type, body) = send(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/markdown"));
    assert!(!body.is_empty());
}

#[tokio::test]
async fn detect_skips_markdown_generation() {
    let request = raw_pdf_request("/v1/detect", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["markdown"].is_null());
    assert_eq!(body["has_text"], false);
    assert!(body["confidence"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn classify_reports_zero_indexed_ocr_pages() {
    let request = raw_pdf_request("/v1/classify", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["pages_needing_ocr"].is_array());
    assert_eq!(body["pdf_type"], "text_based");
}

#[tokio::test]
async fn text_endpoint_can_return_plain_text() {
    let request = raw_pdf_request("/v1/text?format=text", pdf_bytes(TEXT_PDF));
    let (status, content_type, body) = send(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/plain"));
    assert!(!String::from_utf8_lossy(&body).trim().is_empty());
}

#[tokio::test]
async fn items_endpoint_returns_positioned_text() {
    let request = raw_pdf_request("/v1/items?pages=1", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["total_items"].as_u64().unwrap() > 0);
    let first = &body["items"][0];
    assert_eq!(first["page"], 1);
    assert!(first["font_size"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn pages_markdown_returns_one_entry_per_page() {
    let request = raw_pdf_request("/v1/pages/markdown", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    let pages = body["pages"].as_array().unwrap();
    assert!(!pages.is_empty());
    assert_eq!(pages[0]["page"], 0);
}

#[tokio::test]
async fn regions_text_returns_one_result_per_requested_region() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let body = serde_json::json!({
        "pdf_base64": STANDARD.encode(pdf_bytes(TEXT_PDF)),
        "page_regions": [{ "page": 0, "regions": [[0.0, 0.0, 612.0, 792.0]] }],
    });
    let (status, body) = send_json(app(), json_request("/v1/regions/text", body)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pages"][0]["page"], 0);
    assert_eq!(body["pages"][0]["regions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn tsr_endpoint_renders_a_table_per_input() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let body = serde_json::json!({
        "pdf_base64": STANDARD.encode(pdf_bytes(TEXT_PDF)),
        "tables": [{
            "page": 0,
            "crop_pdf_pt_bbox": [0.0, 0.0, 612.0, 792.0],
            "render_dpi": 200.0,
            "structure_tokens": ["<table>", "<tr>", "<td></td>", "<td></td>", "</tr>", "</table>"],
            "cell_bboxes": [[0.0, 0.0, 800.0, 200.0], [800.0, 0.0, 1600.0, 200.0]],
        }],
    });
    let (status, body) = send_json(app(), json_request("/v1/tables/structure/auto", body)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tables"].as_array().unwrap().len(), 1);
    assert!(body["tables"][0]["markdown"].is_string());
}

// -------------------------------------------------------------------------
// Options and errors
// -------------------------------------------------------------------------

#[tokio::test]
async fn password_protected_pdfs_need_the_password() {
    let (status, body) =
        send_json(app(), raw_pdf_request("/v1/process", pdf_bytes(ENCRYPTED_PDF))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "encrypted");

    let request = raw_pdf_request("/v1/process?password=secret123", pdf_bytes(ENCRYPTED_PDF));
    let (status, body) = send_json(app(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["markdown"].as_str().unwrap().contains("Procurement"));
}

#[tokio::test]
async fn non_pdf_input_is_a_content_error() {
    let request = raw_pdf_request("/v1/process", b"this is not a pdf".to_vec());
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "not_a_pdf");
}

#[tokio::test]
async fn missing_body_is_a_client_error() {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/process")
        .header(CONTENT_TYPE, "application/pdf")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn unknown_option_values_are_rejected() {
    let request = raw_pdf_request("/v1/process?mode=turbo", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn zero_page_numbers_are_rejected() {
    let request = raw_pdf_request("/v1/items?pages=0", pdf_bytes(TEXT_PDF));
    let (status, _) = send_json(app(), request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oversized_uploads_are_refused() {
    let config = ServerConfig {
        max_upload_bytes: 1024,
        ..ServerConfig::default()
    };
    let request = raw_pdf_request("/v1/process", pdf_bytes(TEXT_PDF));
    let (status, _, _) = send(app_with(config), request).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

// -------------------------------------------------------------------------
// Authentication
// -------------------------------------------------------------------------

fn authed_config() -> ServerConfig {
    ServerConfig {
        api_key: Some("s3cret".to_string()),
        ..ServerConfig::default()
    }
}

#[tokio::test]
async fn api_key_is_required_when_configured() {
    let request = raw_pdf_request("/v1/detect", pdf_bytes(TEXT_PDF));
    let (status, body) = send_json(app_with(authed_config()), request).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn health_never_requires_an_api_key() {
    let request = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send_json(app_with(authed_config()), request).await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn bearer_and_header_keys_both_authenticate() {
    let mut request = raw_pdf_request("/v1/detect", pdf_bytes(TEXT_PDF));
    request
        .headers_mut()
        .insert(AUTHORIZATION, "Bearer s3cret".parse().unwrap());
    let (status, _) = send_json(app_with(authed_config()), request).await;
    assert_eq!(status, StatusCode::OK);

    let mut request = raw_pdf_request("/v1/detect", pdf_bytes(TEXT_PDF));
    request
        .headers_mut()
        .insert("x-api-key", "s3cret".parse().unwrap());
    let (status, _) = send_json(app_with(authed_config()), request).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_wrong_api_key_is_rejected() {
    let mut request = raw_pdf_request("/v1/detect", pdf_bytes(TEXT_PDF));
    request
        .headers_mut()
        .insert(AUTHORIZATION, "Bearer wrong".parse().unwrap());
    let (status, _) = send_json(app_with(authed_config()), request).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
