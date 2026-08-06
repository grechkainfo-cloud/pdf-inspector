//! Machine-readable description of the HTTP API.
//!
//! Served verbatim at `GET /openapi.json`. Kept as a separate JSON file so it
//! can be linted, diffed, and fed to client generators without extracting it
//! from Rust source.

/// OpenAPI 3.1 description of every endpoint.
pub const OPENAPI_JSON: &str = include_str!("openapi.json");

#[cfg(test)]
mod tests {
    #[test]
    fn openapi_document_is_valid_json_and_covers_every_route() {
        let doc: serde_json::Value =
            serde_json::from_str(super::OPENAPI_JSON).expect("openapi.json must be valid JSON");

        let paths = doc["paths"]
            .as_object()
            .expect("openapi.json must define paths");

        for route in [
            "/health",
            "/version",
            "/v1/process",
            "/v1/detect",
            "/v1/analyze",
            "/v1/classify",
            "/v1/text",
            "/v1/items",
            "/v1/pages/markdown",
            "/v1/regions/text",
            "/v1/regions/tables",
            "/v1/regions/vector-grid",
            "/v1/tables/structure",
            "/v1/tables/structure/cells",
            "/v1/tables/structure/auto",
        ] {
            assert!(paths.contains_key(route), "openapi.json is missing {route}");
        }
    }
}
