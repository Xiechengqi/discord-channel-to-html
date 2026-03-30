use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use include_dir::{Dir, include_dir};

static FRONTEND_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/frontend/out");

pub async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    let file = FRONTEND_DIR
        .get_file(path)
        .or_else(|| FRONTEND_DIR.get_file(format!("{path}.html")))
        .or_else(|| {
            if path.is_empty() {
                FRONTEND_DIR.get_file("index.html")
            } else {
                FRONTEND_DIR
                    .get_file(format!("{path}/index.html"))
                    .or_else(|| FRONTEND_DIR.get_file("index.html"))
            }
        })
        .or_else(|| FRONTEND_DIR.get_file("index.html"));

    let Some(file) = file else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = mime_from_path(file.path().to_str().unwrap_or(""));
    ([(header::CONTENT_TYPE, mime)], file.contents()).into_response()
}

fn mime_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
