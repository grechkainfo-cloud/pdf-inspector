//! Uniform error envelope for the HTTP API.
//!
//! Every failure — bad request, unparsable PDF, worker panic — renders as
//! `{"error": {"code": "...", "message": "..."}}` so callers can branch on a
//! stable machine-readable `code` instead of parsing prose.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::PdfError;

/// An API error carrying its HTTP status and a stable error code.
#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// 400 — the request itself is malformed (bad JSON, missing file part).
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    /// 401 — `PDF_INSPECTOR_API_KEY` is set and the caller did not match it.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    /// 413 — upload exceeds `PDF_INSPECTOR_MAX_UPLOAD_MB`.
    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
    }

    /// 422 — the request was well-formed but the PDF could not be processed.
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    /// 500 — the server failed for a reason unrelated to the input.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.to_string(),
                message: self.message,
            },
        };
        (status, Json(body)).into_response()
    }
}

impl From<PdfError> for ApiError {
    fn from(err: PdfError) -> Self {
        // Content-level failures are the caller's input problem (422); only IO
        // failures point back at the server.
        match err {
            PdfError::Io(ref io) => ApiError::internal(format!("IO error: {io}")),
            PdfError::Parse(ref msg) => {
                ApiError::unprocessable("parse_error", format!("PDF parsing error: {msg}"))
            }
            PdfError::Encrypted => ApiError::unprocessable(
                "encrypted",
                "PDF is encrypted — supply the correct `password`",
            ),
            PdfError::InvalidStructure => {
                ApiError::unprocessable("invalid_structure", "Invalid PDF structure")
            }
            PdfError::NotAPdf(ref msg) => {
                ApiError::unprocessable("not_a_pdf", format!("Not a PDF: {msg}"))
            }
        }
    }
}
