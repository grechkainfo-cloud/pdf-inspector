//! Browser console for exercising the API by hand.
//!
//! The page is compiled into the binary and served from the same origin as
//! the API, so it needs no CORS handling, no build step, and no network at
//! runtime — it works inside an air-gapped deployment exactly as it does on
//! a laptop. Disable it with `PDF_INSPECTOR_UI=0`.

use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::response::{IntoResponse, Response};

/// The single-file console: markup, styles, and script.
pub const UI_HTML: &str = include_str!("ui.html");

/// The page is self-contained by design, so the policy can be strict: it may
/// talk to its own origin and nowhere else. `unsafe-inline` is required
/// because the CSS and JS live in the document rather than in separate
/// files — a deliberate trade for a page that ships as one compiled string.
///
/// `blob:` stays in `default-src` because downloading a result builds an
/// object URL: the explicit directives below already pin fetches, scripts,
/// and styles to this origin, so the allowance only covers the download.
const UI_CSP: &str = "default-src 'self' blob:; \
     script-src 'self' 'unsafe-inline'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; \
     connect-src 'self'; \
     form-action 'none'; \
     base-uri 'none'; \
     frame-ancestors 'none'";

/// Render the console with its security headers.
pub fn response() -> Response {
    (
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (CONTENT_SECURITY_POLICY, UI_CSP),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        UI_HTML,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::UI_HTML;

    #[test]
    fn console_loads_nothing_from_the_network() {
        // An air-gapped server must still render the page in full, so any
        // absolute URL in markup or CSS would be a regression.
        for needle in [
            "src=\"http",
            "src='http",
            "href=\"http://",
            "href=\"https://",
            "@import",
            "cdn.",
            "unpkg.com",
            "googleapis.com",
        ] {
            assert!(
                !UI_HTML.contains(needle),
                "console references an external resource: {needle}"
            );
        }
    }

    #[test]
    fn console_calls_every_documented_endpoint() {
        for path in [
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
            "/v1/tables/structure/auto",
            "/v1/tables/structure/cells",
        ] {
            assert!(
                UI_HTML.contains(&format!("\"{path}\"")),
                "console has no entry for {path}"
            );
        }
    }
}
