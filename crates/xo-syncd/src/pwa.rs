use bytes::Bytes;
use http_body_util::Full;
use hyper::{Response, StatusCode};

pub type Body = Full<Bytes>;

pub struct EmbeddedAsset {
    path: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/embedded_pwa.rs"));

pub fn serve_head(path: &str) -> Response<Body> {
    serve(path).map(|_| Full::new(Bytes::new()))
}

pub fn serve(path: &str) -> Response<Body> {
    let requested = path.strip_prefix('/').unwrap_or(path);
    let asset = find(requested).or_else(|| {
        let has_extension = requested
            .rsplit_once('/')
            .map_or(requested, |(_, name)| name)
            .contains('.');
        (!has_extension).then(|| find("index.html")).flatten()
    });
    let Some(asset) = asset else {
        return response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "Not found\n",
            "no-store",
        );
    };
    let cache_control = if asset.path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response(
        StatusCode::OK,
        content_type(asset.path),
        asset.bytes,
        cache_control,
    )
}

fn find(path: &str) -> Option<&'static EmbeddedAsset> {
    let path = if path.is_empty() { "index.html" } else { path };
    EMBEDDED_PWA.iter().find(|asset| asset.path == path)
}

fn response(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Bytes>,
    cache_control: &'static str,
) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("cache-control", cache_control)
        .header("x-content-type-options", "nosniff")
        .body(Full::new(body.into()))
        .expect("static PWA response is valid")
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_spa_routes_use_embedded_index_without_masking_missing_assets() {
        let root = serve("/");
        assert_eq!(root.status(), StatusCode::OK);
        assert_eq!(root.headers()["cache-control"], "no-cache");
        assert_eq!(serve("/notes/abcdefg").status(), StatusCode::OK);
        assert_eq!(serve("/missing.js").status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn cache_and_content_type_rules_are_stable() {
        let script = serve("/assets/app-dev.js");
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(
            script.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            script.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(serve("/sw.js").headers()["cache-control"], "no-cache");
        assert_eq!(content_type("runtime.wasm"), "application/wasm");
        assert_eq!(
            content_type("manifest.webmanifest"),
            "application/manifest+json"
        );
    }
}
