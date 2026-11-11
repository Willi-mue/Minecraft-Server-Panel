//! Serving files from a folder (panel frontend, map viewer, map tiles) with ETag / Last-Modified,
//! so browsers revalidate cheaply (304).

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use md5::{Digest, Md5};
use tokio_util::io::ReaderStream;

pub fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("zip") => "application/zip",
        Some("bin") => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

/// `rel` inside `base`, or None for anything that could leave it.
pub fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    if rel.contains('\\') || rel.contains('\0') {
        return None;
    }
    let mut out = base.to_path_buf();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(p) => {
                let s = p.to_string_lossy();
                if s.contains(':') {
                    return None;
                }
                out.push(p);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

pub fn etag_of(meta: &std::fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    format!(
        "\"{}\"",
        hex::encode(Md5::digest(format!("{mtime}-{}", meta.len()).as_bytes()))
    )
}

fn http_date(meta: &std::fs::Metadata) -> Option<String> {
    let t: chrono::DateTime<chrono::Utc> = meta.modified().ok()?.into();
    Some(t.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
}

/// A file response (streamed), 304 when the browser's copy is current, None if it doesn't exist.
pub async fn file(path: &Path, req: &HeaderMap, cache_control: &str) -> Option<Response> {
    let meta = tokio::fs::metadata(path)
        .await
        .ok()
        .filter(|m| m.is_file())?;
    let etag = etag_of(&meta);
    let mut headers = HeaderMap::new();
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).ok()?);
    if let Some(d) = http_date(&meta).and_then(|d| HeaderValue::from_str(&d).ok()) {
        headers.insert(header::LAST_MODIFIED, d);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(cache_control).ok()?,
    );
    let not_modified = req
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == etag)
        });
    if not_modified {
        return Some((StatusCode::NOT_MODIFIED, headers).into_response());
    }
    let f = tokio::fs::File::open(path).await.ok()?;
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
    Some(
        (
            headers,
            Body::from_stream(ReaderStream::with_capacity(f, 1 << 16)),
        )
            .into_response(),
    )
}

/// A file as a download (`Content-Disposition: attachment`).
pub async fn download(path: &Path, filename: &str) -> Option<Response> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let f = tokio::fs::File::open(path).await.ok()?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
    let safe: String = filename
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .collect();
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")).ok()?,
    );
    Some(
        (
            headers,
            Body::from_stream(ReaderStream::with_capacity(f, 1 << 16)),
        )
            .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_stay_inside() {
        let base = Path::new("/srv/map");
        assert_eq!(
            safe_join(base, "a/b.png"),
            Some(base.join("a").join("b.png"))
        );
        assert_eq!(safe_join(base, "/a"), Some(base.join("a")));
        assert_eq!(safe_join(base, ""), Some(base.to_path_buf()));
        assert_eq!(safe_join(base, "../x"), None);
        assert_eq!(safe_join(base, "a/../../x"), None);
        assert_eq!(safe_join(base, "a\\..\\x"), None);
        assert_eq!(safe_join(base, "C:/x"), None);
    }
}
