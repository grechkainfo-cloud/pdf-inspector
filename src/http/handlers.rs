//! Endpoint handlers.
//!
//! Every handler is a thin wrapper: parse options → hand the bytes to the
//! library on a blocking worker → map the result onto a DTO. Extraction is
//! CPU-bound and can take seconds on large documents, so it never runs on the
//! async runtime's threads.

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::dto::*;
use super::error::ApiError;
use super::payload::PdfPayload;
use super::AppState;
use crate::ProcessMode;

// =========================================================================
// Service endpoints
// =========================================================================

/// `GET /` — endpoint directory, so a bare `curl` of the root is useful.
pub async fn index() -> impl IntoResponse {
    Json(serde_json::json!({
        "service": "pdf-inspector",
        "version": env!("CARGO_PKG_VERSION"),
        "openapi": "/openapi.json",
        "endpoints": {
            "GET  /health": "liveness probe",
            "GET  /version": "build info",
            "GET  /openapi.json": "OpenAPI 3.1 description of this API",
            "POST /v1/process": "detect + extract + markdown (full pipeline)",
            "POST /v1/detect": "type detection only (fast, no extraction)",
            "POST /v1/analyze": "detect + extract + layout analysis (no markdown)",
            "POST /v1/classify": "lightweight routing classification",
            "POST /v1/text": "plain text extraction",
            "POST /v1/items": "positioned text items with font metadata",
            "POST /v1/pages/markdown": "per-page markdown + layout metadata",
            "POST /v1/regions/text": "text inside bounding-box regions",
            "POST /v1/regions/tables": "markdown tables inside bounding-box regions",
            "POST /v1/regions/vector-grid": "detect a ruled-line grid in a region",
            "POST /v1/tables/structure": "TSR-driven table markdown",
            "POST /v1/tables/structure/cells": "TSR-driven structured cells",
            "POST /v1/tables/structure/auto": "TSR-driven markdown with quality fallback"
        }
    }))
}

/// `GET /health` — liveness/readiness probe. Never requires an API key.
pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// `GET /version`
pub async fn version() -> impl IntoResponse {
    Json(serde_json::json!({
        "name": "pdf-inspector",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": "v1",
    }))
}

/// `GET /openapi.json`
pub async fn openapi() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/json")],
        super::openapi::OPENAPI_JSON,
    )
}

// =========================================================================
// Full-document endpoints
// =========================================================================

/// `POST /v1/process` — the full pipeline (detect → extract → markdown).
pub async fn process(state: State<AppState>, payload: PdfPayload) -> Result<Response, ApiError> {
    run_process(state, payload, ProcessMode::Full).await
}

/// `POST /v1/detect` — metadata-only detection.
pub async fn detect(state: State<AppState>, payload: PdfPayload) -> Result<Response, ApiError> {
    run_process(state, payload, ProcessMode::DetectOnly).await
}

/// `POST /v1/analyze` — detection plus layout complexity, no markdown.
pub async fn analyze(state: State<AppState>, payload: PdfPayload) -> Result<Response, ApiError> {
    run_process(state, payload, ProcessMode::Analyze).await
}

async fn run_process(
    State(state): State<AppState>,
    payload: PdfPayload,
    default_mode: ProcessMode,
) -> Result<Response, ApiError> {
    let options: ProcessOptions = payload.options()?;
    let wants_markdown = options.format.as_deref() == Some("markdown");
    let pdf_options = options.to_pdf_options(default_mode)?;
    let pdf = payload.pdf;

    let result = state
        .run_blocking(move || crate::process_pdf_mem_with_options(&pdf, pdf_options))
        .await?;

    if wants_markdown {
        return Ok(markdown_response(result.markdown.unwrap_or_default()));
    }
    Ok(Json(ProcessResponse::from(result)).into_response())
}

/// `POST /v1/classify` — cheap routing decision (type + OCR pages).
pub async fn classify(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<ClassifyResponse>, ApiError> {
    let pdf = payload.pdf;
    let result = state
        .run_blocking(move || crate::classify_pdf_mem(&pdf))
        .await?;
    Ok(Json(ClassifyResponse::from(result)))
}

/// `POST /v1/text` — plain text, no markdown structuring.
pub async fn text(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Response, ApiError> {
    let options: ExtractOptions = payload.options()?;
    let wants_plain = options.format.as_deref() == Some("text");
    let password = options.password.clone();
    let pdf = payload.pdf;

    let text = state
        .run_blocking(move || {
            crate::extractor::extract_text_mem_with_password(&pdf, password.as_deref())
        })
        .await?;

    if wants_plain {
        return Ok(([(CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response());
    }
    Ok(Json(TextResponse {
        length: text.len(),
        text,
    })
    .into_response())
}

/// `POST /v1/items` — positioned text items with font and style metadata.
pub async fn items(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<ItemsResponse>, ApiError> {
    let options: ExtractOptions = payload.options()?;
    let page_filter = options.page_filter()?;
    let password = options.password.clone();
    let pdf = payload.pdf;

    let items = state
        .run_blocking(move || {
            crate::extractor::extract_text_with_positions_mem_pages_with_password(
                &pdf,
                page_filter.as_ref(),
                password.as_deref(),
            )
        })
        .await?;
    Ok(Json(ItemsResponse::from(items)))
}

/// `POST /v1/pages/markdown` — per-page markdown for hybrid OCR pipelines.
pub async fn pages_markdown(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<PagesMarkdownResponse>, ApiError> {
    let options: PagesMarkdownOptions = payload.options()?;
    let pages = options.resolve_pages()?;
    let pdf = payload.pdf;

    let result = state
        .run_blocking(move || crate::extract_pages_markdown_mem(&pdf, pages.as_deref()))
        .await?;
    Ok(Json(PagesMarkdownResponse::from(result)))
}

// =========================================================================
// Region endpoints
// =========================================================================

/// `POST /v1/regions/text` — text inside layout-model bounding boxes.
pub async fn regions_text(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<RegionsResponse>, ApiError> {
    let options: RegionsOptions = payload.options()?;
    let regions = options.to_page_regions()?;
    let pdf = payload.pdf;

    let result = state
        .run_blocking(move || crate::extract_text_in_regions_mem(&pdf, &regions))
        .await?;
    Ok(Json(RegionsResponse::from(result)))
}

/// `POST /v1/regions/tables` — markdown pipe-tables inside bounding boxes.
pub async fn regions_tables(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<RegionsResponse>, ApiError> {
    let options: RegionsOptions = payload.options()?;
    let regions = options.to_page_regions()?;
    let pdf = payload.pdf;

    let result = state
        .run_blocking(move || crate::extract_tables_in_regions_mem(&pdf, &regions))
        .await?;
    Ok(Json(RegionsResponse::from(result)))
}

/// `POST /v1/regions/vector-grid` — ruled-line/rect grid inside one region.
pub async fn regions_vector_grid(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<VectorGridResponse>, ApiError> {
    let options: VectorGridOptions = payload.options()?;
    validate_bbox(&options.region, "region")?;
    if !options.render_dpi.is_finite() || options.render_dpi <= 0.0 {
        return Err(ApiError::bad_request(
            "render_dpi must be a positive number",
        ));
    }
    let pdf = payload.pdf;

    let result = state
        .run_blocking(move || {
            crate::detect_vector_grid_in_region_mem(
                &pdf,
                options.page,
                options.region,
                options.render_dpi,
            )
        })
        .await?;
    Ok(Json(VectorGridResponse::from(result)))
}

// =========================================================================
// TSR table endpoints
// =========================================================================

/// `POST /v1/tables/structure` — raw TSR markdown, no quality fallback.
pub async fn tables_structure(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<TablesMarkdownResponse>, ApiError> {
    let options: TsrOptions = payload.options()?;
    let inputs = options.to_inputs()?;
    let pdf = payload.pdf;

    let markdowns = state
        .run_blocking(move || crate::extract_tables_with_structure_mem(&pdf, &inputs))
        .await?;
    Ok(Json(TablesMarkdownResponse {
        tables: markdowns
            .into_iter()
            .map(|markdown| TableMarkdownDto {
                markdown,
                fallback_reason: None,
            })
            .collect(),
    }))
}

/// `POST /v1/tables/structure/cells` — TSR cells with spans and bboxes.
pub async fn tables_structure_cells(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<TableCellsResponse>, ApiError> {
    let options: TsrOptions = payload.options()?;
    let inputs = options.to_inputs()?;
    let pdf = payload.pdf;

    let cells = state
        .run_blocking(move || crate::extract_tables_with_structure_cells_mem(&pdf, &inputs))
        .await?;
    Ok(Json(TableCellsResponse::from(cells)))
}

/// `POST /v1/tables/structure/auto` — TSR markdown with self-healing fallback.
pub async fn tables_structure_auto(
    State(state): State<AppState>,
    payload: PdfPayload,
) -> Result<Json<TablesMarkdownResponse>, ApiError> {
    let options: TsrOptions = payload.options()?;
    let inputs = options.to_inputs()?;
    let pdf = payload.pdf;

    let results = state
        .run_blocking(move || crate::extract_tables_with_structure_auto_mem(&pdf, &inputs))
        .await?;
    Ok(Json(TablesMarkdownResponse {
        tables: results
            .into_iter()
            .map(|result| TableMarkdownDto {
                markdown: result.markdown,
                fallback_reason: result.fallback_reason,
            })
            .collect(),
    }))
}

// =========================================================================
// Helpers
// =========================================================================

fn markdown_response(markdown: String) -> Response {
    (
        [(CONTENT_TYPE, "text/markdown; charset=utf-8")],
        markdown,
    )
        .into_response()
}

/// `GET`/`POST` on an unknown path.
pub async fn not_found() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::NOT_FOUND,
        "not_found",
        "unknown endpoint — see GET / for the endpoint directory",
    )
}
