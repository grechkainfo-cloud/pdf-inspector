//! Request body handling.
//!
//! Every PDF endpoint accepts the same three shapes so callers can pick what
//! their client makes easy:
//!
//! 1. `multipart/form-data` — a `file` part plus optional option parts (either
//!    one JSON `options` part or flat text parts).
//! 2. `application/json` — `{"pdf_base64": "...", ...options}`.
//! 3. any other content type — the raw body is the PDF, options come from the
//!    query string.
//!
//! Whichever shape is used, the handler sees the same `(bytes, options)` pair.

use axum::body::Bytes;
use axum::extract::{FromRequest, Multipart, Query, Request};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::HashMap;

use super::error::ApiError;
use super::AppState;

/// Multipart part names accepted for the PDF itself.
const FILE_FIELDS: &[&str] = &["file", "pdf", "document", "upload"];
/// Multipart part names whose body is a JSON object of options.
const JSON_FIELDS: &[&str] = &["options", "payload", "json", "body"];
/// Keys that must stay strings even when their text parses as JSON — a
/// password of `1234` must not arrive as the number 1234.
const STRING_KEYS: &[&str] = &["password", "mode", "profile", "format", "strategy"];

/// A PDF plus the request options, normalised across all three body shapes.
pub struct PdfPayload {
    /// Raw PDF bytes.
    pub pdf: Vec<u8>,
    /// Options as a JSON object (never any other JSON kind).
    pub options: Value,
}

impl PdfPayload {
    /// Deserialize the options into an endpoint-specific struct.
    ///
    /// Unknown keys are ignored so a single query string can carry options for
    /// clients that talk to several endpoints.
    pub fn options<T: DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_value(self.options.clone())
            .map_err(|e| ApiError::bad_request(format!("invalid options: {e}")))
    }
}

impl FromRequest<AppState> for PdfPayload {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        // Reject oversized uploads before buffering them. Requests without a
        // Content-Length (chunked) are still capped by the router's
        // DefaultBodyLimit layer.
        if let Some(len) = req
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
        {
            if len > state.max_upload_bytes {
                return Err(ApiError::payload_too_large(format!(
                    "request body is {len} bytes, limit is {} bytes",
                    state.max_upload_bytes
                )));
            }
        }

        // Query parameters are the lowest-priority option source; body fields
        // with the same name win.
        let mut options = query_options(&req)?;

        let pdf = if content_type.starts_with("multipart/form-data") {
            read_multipart(req, state, &mut options).await?
        } else if content_type.starts_with("application/json") {
            read_json(req, state, &mut options).await?
        } else {
            read_raw(req, state).await?
        };

        if pdf.is_empty() {
            return Err(ApiError::bad_request(
                "no PDF in request: send multipart `file`, JSON `pdf_base64`, or a raw PDF body",
            ));
        }

        Ok(Self {
            pdf,
            options: Value::Object(options),
        })
    }
}

/// Parse the query string into an option map.
fn query_options(req: &Request) -> Result<Map<String, Value>, ApiError> {
    let Query(raw): Query<HashMap<String, String>> = Query::try_from_uri(req.uri())
        .map_err(|e| ApiError::bad_request(format!("invalid query string: {e}")))?;

    let mut options = Map::with_capacity(raw.len());
    for (key, value) in raw {
        let coerced = coerce(&key, value);
        options.insert(key, coerced);
    }
    Ok(options)
}

/// Interpret a text form/query value as JSON when it looks like JSON, so
/// `?pages=[1,2]` and `?compact=true` behave like their JSON-body twins.
fn coerce(key: &str, raw: String) -> Value {
    if STRING_KEYS.contains(&key) {
        return Value::String(raw);
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value,
        Err(_) => Value::String(raw),
    }
}

async fn read_multipart(
    req: Request,
    state: &AppState,
    options: &mut Map<String, Value>,
) -> Result<Vec<u8>, ApiError> {
    let mut multipart = Multipart::from_request(req, state)
        .await
        .map_err(|e| ApiError::bad_request(format!("invalid multipart body: {e}")))?;

    let mut pdf: Option<Vec<u8>> = None;

    // `name()`/`file_name()` borrow the field, and reading its body consumes
    // it — copy the metadata out first.
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("invalid multipart field: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let is_file = field.file_name().is_some();

        if FILE_FIELDS.contains(&name.as_str()) || (is_file && pdf.is_none()) {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| ApiError::bad_request(format!("could not read `{name}` part: {e}")))?;
            pdf = Some(bytes.to_vec());
            continue;
        }

        let text = field
            .text()
            .await
            .map_err(|e| ApiError::bad_request(format!("could not read `{name}` part: {e}")))?;

        if JSON_FIELDS.contains(&name.as_str()) {
            let parsed: Value = serde_json::from_str(&text)
                .map_err(|e| ApiError::bad_request(format!("invalid JSON in `{name}`: {e}")))?;
            let Value::Object(map) = parsed else {
                return Err(ApiError::bad_request(format!(
                    "`{name}` must be a JSON object"
                )));
            };
            for (key, value) in map {
                options.insert(key, value);
            }
        } else if !name.is_empty() {
            let coerced = coerce(&name, text);
            options.insert(name, coerced);
        }
    }

    pdf.ok_or_else(|| ApiError::bad_request("multipart request has no `file` part"))
}

async fn read_json(
    req: Request,
    state: &AppState,
    options: &mut Map<String, Value>,
) -> Result<Vec<u8>, ApiError> {
    let bytes = read_raw(req, state).await?;
    let parsed: Value = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))?;
    let Value::Object(mut map) = parsed else {
        return Err(ApiError::bad_request("JSON body must be an object"));
    };

    let encoded = ["pdf_base64", "pdf_b64", "pdf", "file", "data", "content"]
        .iter()
        .find_map(|key| map.remove(*key))
        .ok_or_else(|| ApiError::bad_request("JSON body must contain `pdf_base64`"))?;

    let Value::String(encoded) = encoded else {
        return Err(ApiError::bad_request("`pdf_base64` must be a string"));
    };

    for (key, value) in map {
        options.insert(key, value);
    }

    decode_base64(&encoded)
}

async fn read_raw(req: Request, state: &AppState) -> Result<Vec<u8>, ApiError> {
    let bytes = Bytes::from_request(req, state)
        .await
        .map_err(|e| ApiError::payload_too_large(format!("could not read request body: {e}")))?;
    if bytes.len() > state.max_upload_bytes {
        return Err(ApiError::payload_too_large(format!(
            "request body is {} bytes, limit is {} bytes",
            bytes.len(),
            state.max_upload_bytes
        )));
    }
    Ok(bytes.to_vec())
}

/// Decode base64, tolerating `data:` URL prefixes, embedded whitespace, and
/// missing padding — all common in hand-rolled clients.
fn decode_base64(encoded: &str) -> Result<Vec<u8>, ApiError> {
    let body = match encoded.find(";base64,") {
        Some(idx) if encoded.starts_with("data:") => &encoded[idx + ";base64,".len()..],
        _ => encoded,
    };
    let cleaned: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();

    STANDARD
        .decode(&cleaned)
        .or_else(|_| STANDARD_NO_PAD.decode(cleaned.trim_end_matches('=')))
        .map_err(|e| ApiError::bad_request(format!("`pdf_base64` is not valid base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coerce_keeps_secret_like_values_as_strings() {
        assert_eq!(
            coerce("password", "1234".to_string()),
            Value::String("1234".to_string())
        );
        assert_eq!(coerce("compact", "true".to_string()), Value::Bool(true));
    }

    #[test]
    fn coerce_parses_json_arrays_but_keeps_page_specs() {
        assert_eq!(
            coerce("pages", "[1,2]".to_string()),
            Value::Array(vec![Value::from(1), Value::from(2)])
        );
        assert_eq!(
            coerce("pages", "1,3-5".to_string()),
            Value::String("1,3-5".to_string())
        );
    }

    #[test]
    fn base64_accepts_data_urls_and_whitespace() {
        let plain = STANDARD.encode(b"%PDF-1.4");
        let spaced = format!("{}\n{}", &plain[..4], &plain[4..]);
        assert_eq!(decode_base64(&spaced).unwrap(), b"%PDF-1.4".to_vec());

        let data_url = format!("data:application/pdf;base64,{plain}");
        assert_eq!(decode_base64(&data_url).unwrap(), b"%PDF-1.4".to_vec());
    }

    #[test]
    fn base64_rejects_garbage() {
        assert!(decode_base64("not base64 !!!").is_err());
    }
}
