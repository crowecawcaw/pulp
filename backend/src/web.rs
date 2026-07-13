//! Embedded frontend serving — the "single binary" half of deployment.
//!
//! `backend/web-dist` is compiled into release binaries via rust-embed (debug
//! builds read the directory from disk live, so `npm run build` is picked up
//! without recompiling). Vite writes the built UI there (see
//! `frontend/vite.config.ts`); keeping it inside the crate root lets
//! `cargo package`/`cargo install` vendor it. The handler is installed as the
//! router's fallback:
//! anything the API doesn't match is served as a static asset, with unmatched
//! extensionless paths falling back to `index.html` for client-side routing.

use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web-dist"]
struct FrontendAssets;

/// Shown at `/` when the binary was built without a frontend (empty dist).
const PLACEHOLDER: &str = "<!doctype html><html><head><title>Pulp</title></head><body>\
<h1>Pulp API is running</h1>\
<p>The web UI was not bundled into this build. Build it with\
 <code>cd frontend &amp;&amp; npm run build</code> and restart (release builds\
 embed it; debug builds serve it live from <code>backend/web-dist</code>).</p>\
</body></html>";

/// Router fallback: serve the embedded SPA.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // The API never falls through to the SPA shell: a wrong /api path must
    // 404, not return index.html (which would confuse JSON clients).
    if path == "api" || path.starts_with("api/") || path.starts_with("api-docs") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let candidate = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = FrontendAssets::get(candidate) {
        return asset_response(candidate, file.data);
    }

    // Asset-looking paths (an extension) 404; route-looking paths get the SPA
    // shell so client-side routes survive a hard refresh.
    let last_segment = path.rsplit('/').next().unwrap_or(path);
    if last_segment.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match FrontendAssets::get("index.html") {
        Some(file) => asset_response("index.html", file.data),
        None => Html(PLACEHOLDER).into_response(),
    }
}

fn asset_response(path: &str, data: std::borrow::Cow<'static, [u8]>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Vite content-hashes everything under assets/, so those are immutable;
    // everything else (index.html, sw.js, manifest, icons) must revalidate or
    // service-worker updates would never reach installed PWAs.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        data,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The /api guard and extension heuristics are pure routing decisions;
    /// exercise them through the handler. (Whether real dist files exist
    /// depends on the build machine, so tests only assert the invariants that
    /// hold either way.)
    #[tokio::test]
    async fn api_paths_never_get_the_spa_shell() {
        for path in ["/api", "/api/", "/api/definitely-missing", "/api-docs/x"] {
            let resp = static_handler(path.parse::<Uri>().unwrap()).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "path: {path}");
        }
    }

    #[tokio::test]
    async fn root_and_client_routes_always_resolve() {
        // Either the built index.html or the placeholder — never a 404.
        for path in ["/", "/feed", "/monitors/123"] {
            let resp = static_handler(path.parse::<Uri>().unwrap()).await;
            assert_eq!(resp.status(), StatusCode::OK, "path: {path}");
        }
    }

    #[tokio::test]
    async fn missing_assets_404_rather_than_masquerade_as_html() {
        let resp = static_handler("/definitely-missing.png".parse::<Uri>().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
