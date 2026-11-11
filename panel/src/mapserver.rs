//! Public live map web server (separate port from the admin panel).
//!
//! Serves the map viewer (app/static/map), the tiles rendered by `mapgen` and the live player
//! positions. Nothing else: no panel API, no login. Works while Minecraft is stopped.
//!
//! Every server has its own map at `/s/<id>/` (the whole viewer is served there too, so its
//! relative URLs need no prefix); `/` serves the first server (`main`), the viewer there moves on to
//! the server the browser showed last. `/servers.json` lists the servers whose map is turned on, for
//! the viewer's server switch.
//!
//! The only writes are waypoints (`waypoints.rs`): small JSON bodies, strictly validated, and only
//! from the map page itself (JSON content type + same Origin, so other web pages a player visits
//! cannot use their address-based recognition).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use serde_json::{Map, Value, json};

use crate::config::{self, cfg};
use crate::instances::{self, Instance};
use crate::util::lock;
use crate::waypoints::{self, Visitor, WpError};
use crate::{leaderboard, livemap, skins, statics};

const PLACEHOLDER: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Live map</title>
<style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#1c1f26;color:#dde2ea;
font:15px system-ui,sans-serif;text-align:center;padding:16px}p{color:#959eae}</style></head>
<body><div><h1>Live map</h1><p>{text}</p></div></body></html>"#;

// 1×1 transparent PNG
static EMPTY_PNG: LazyLock<Vec<u8>> = LazyLock::new(|| {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")
        .unwrap()
});

const BODY_MAX: usize = 4096;

static ENABLED_CACHE: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> =
    LazyLock::new(Default::default);
/// blocks.json is a few MB of repetitive JSON: sent gzipped, compressed once per file version
static GZIPPED: LazyLock<Mutex<HashMap<String, (u128, Bytes)>>> = LazyLock::new(Default::default);

fn viewer_dir() -> PathBuf {
    cfg().static_dir.join("map")
}

/// settings.json is re-read at most every 5 s (tile requests come in bursts).
fn map_enabled(inst: &Instance) -> bool {
    if let Some((t, v)) = lock(&ENABLED_CACHE).get(&inst.id) {
        if t.elapsed() < Duration::from_secs(5) {
            return *v;
        }
    }
    let v = config::load_settings(inst).map_enabled;
    lock(&ENABLED_CACHE).insert(inst.id.clone(), (Instant::now(), v));
    v
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "Not found",
    )
        .into_response()
}

fn off() -> Response {
    let html = PLACEHOLDER.replace("{text}", "The live map is turned off.");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn json_response(data: &Value, status: StatusCode, cache: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, cache),
        ],
        data.to_string(),
    )
        .into_response()
}

/// Cross-origin isolation: lets the 3D viewer share decoded regions between its web workers
/// (SharedArrayBuffer) instead of copying them. Everything the viewer loads is same-origin.
fn isolate(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "cross-origin-embedder-policy",
        HeaderValue::from_static("require-corp"),
    );
    r
}

pub async fn handle(ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request) -> Response {
    isolate(dispatch(addr, req).await)
}

async fn dispatch(addr: SocketAddr, req: Request) -> Response {
    let path = req.uri().path().to_string();
    // `/s/<id>` + rest, else main
    let (inst, rest) = match path.strip_prefix("/s/") {
        Some(after) => {
            let (sid, rest) = match after.split_once('/') {
                Some((sid, rest)) => (sid, Some(rest)),
                None => (after, None),
            };
            let sid = percent(sid);
            let Ok(inst) = instances::validate_id(&sid).and_then(instances::get) else {
                return not_found();
            };
            let Some(rest) = rest else {
                return Redirect::temporary(&format!("{path}/")).into_response();
            };
            (inst, percent(rest))
        }
        None => (Instance::main(), percent(path.trim_start_matches('/'))),
    };
    let method = req.method().clone();
    let address = addr.ip().to_string();

    match (method.clone(), rest.as_str()) {
        (Method::GET | Method::HEAD, "maps.json") => maps_json(&inst).await,
        (Method::GET | Method::HEAD, "players.json") => players_json(&inst),
        (Method::GET | Method::HEAD, "stats.json") => stats_json(&inst).await,
        (Method::GET | Method::HEAD, "me.json") => me_json(&inst, &address),
        (Method::GET | Method::HEAD, "servers.json") => servers_json().await,
        (Method::GET | Method::HEAD, "waypoints.json") => {
            waypoints_json(&inst, req.headers(), &address).await
        }
        (Method::POST, "waypoints") => waypoint_write(&inst, None, req, &address).await,
        (Method::PUT | Method::DELETE, r)
            if r.starts_with("waypoints/") && !r[10..].contains('/') =>
        {
            let wid = r[10..].to_string();
            waypoint_write(&inst, Some(wid), req, &address).await
        }
        (Method::GET | Method::HEAD, "blocks.json" | "atlas.png") => {
            asset_file(&inst, &rest, req.headers()).await
        }
        (Method::GET | Method::HEAD, r) if r.starts_with("skins/") && r.ends_with(".png") => {
            skin_png(&inst, &r[6..r.len() - 4]).await
        }
        (Method::GET | Method::HEAD, r) if r.starts_with("heads/") && r.ends_with(".png") => {
            head_png(&inst, &r[6..r.len() - 4]).await
        }
        (Method::GET | Method::HEAD, r) if r.starts_with("tiles/") => {
            tiles(&inst, &r[6..], req.headers()).await
        }
        (Method::GET | Method::HEAD, r) => viewer(&inst, r, req.headers()).await,
        _ => (StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response(),
    }
}

fn percent(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

async fn viewer(inst: &Instance, path: &str, headers: &HeaderMap) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let Some(mut target) = statics::safe_join(&viewer_dir(), path) else {
        return not_found();
    };
    if target.is_dir() {
        target = target.join("index.html");
    }
    if target.file_name().is_some_and(|n| n == "index.html") {
        // versioned favicon URL: browsers keep favicons cached far longer than the page
        if let Ok(html) = std::fs::read_to_string(&target) {
            let v = crate::api::asset_version("map/logo.svg");
            return (
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "public, max-age=300"),
                ],
                html.replace(r#"href="logo.svg""#, &format!(r#"href="logo.svg?v={v}""#)),
            )
                .into_response();
        }
    }
    statics::file(&target, headers, "public, max-age=300")
        .await
        .unwrap_or_else(not_found)
}

async fn tiles(inst: &Instance, path: &str, headers: &HeaderMap) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let base = livemap::out_dir(inst);
    // Tiles change with every render: always revalidate (cheap 304 via ETag)
    if let Some(target) = statics::safe_join(&base, path) {
        if let Some(r) = statics::file(&target, headers, "no-cache").await {
            return r;
        }
    }
    if path.ends_with(".png") {
        // Areas without generated chunks: an empty tile instead of a 404 in the browser console
        return (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            EMPTY_PNG.clone(),
        )
            .into_response();
    }
    not_found()
}

async fn servers_json() -> Response {
    let list = tokio::task::spawn_blocking(|| {
        instances::all_entries()
            .into_iter()
            .filter_map(|e| {
                let inst = Instance::new(&e.id);
                (map_enabled(&inst) && livemap::available(&inst))
                    .then(|| json!({"id": e.id, "name": e.name, "path": format!("s/{}/", e.id)}))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    json_response(&json!({"servers": list}), StatusCode::OK, "no-cache")
}

async fn maps_json(inst: &Instance) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let body = tokio::fs::read(livemap::out_dir(inst).join("maps.json"))
        .await
        .unwrap_or_else(|_| br#"{"dimensions": []}"#.to_vec());
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// Texture tables written by mapgen next to the tiles (only with block textures). The viewer asks
/// for them with ?v=<stamp>, so they can be cached for long.
async fn asset_file(inst: &Instance, name: &str, headers: &HeaderMap) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let path = livemap::out_dir(inst).join(name);
    let Ok(meta) = tokio::fs::metadata(&path).await else {
        return not_found();
    };
    let ctype = if name.ends_with(".json") {
        "application/json"
    } else {
        "image/png"
    };
    let gzip_ok = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    let base = [
        (header::CONTENT_TYPE, ctype),
        (header::CACHE_CONTROL, "public, max-age=86400"),
        (header::VARY, "Accept-Encoding"),
    ];
    if name.ends_with(".json") && gzip_ok {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let key = format!("{}/{name}", inst.id);
        let cached = lock(&GZIPPED)
            .get(&key)
            .filter(|c| c.0 == mtime)
            .map(|c| c.1.clone());
        let body = match cached {
            Some(b) => b,
            None => {
                let p = path.clone();
                let Ok(Ok(raw)) = tokio::task::spawn_blocking(move || std::fs::read(p)).await
                else {
                    return not_found();
                };
                let gz = Bytes::from(
                    tokio::task::spawn_blocking(move || {
                        use std::io::Write;
                        let mut e =
                            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
                        let _ = e.write_all(&raw);
                        e.finish().unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default(),
                );
                lock(&GZIPPED).insert(key, (mtime, gz.clone()));
                gz
            }
        };
        return (base, [(header::CONTENT_ENCODING, "gzip")], body).into_response();
    }
    match tokio::fs::read(&path).await {
        Ok(body) => (base, body).into_response(),
        Err(_) => not_found(),
    }
}

/// A player's skin (downloaded from Mojang by the panel), for the heads on the map.
async fn skin_png(inst: &Instance, uuid: &str) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let Ok(path) = skins::skin_path(uuid) else {
        return not_found();
    };
    match tokio::fs::read(&path).await {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            body,
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

/// Skin of a player head placed in the world (by texture hash, downloaded by the panel).
async fn head_png(inst: &Instance, skin: &str) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let Ok(path) = skins::head_skin_path(skin) else {
        return not_found();
    };
    match tokio::fs::read(&path).await {
        // a texture hash always shows the same picture
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=604800"),
            ],
            body,
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

fn players_json(inst: &Instance) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let tracker = livemap::tracker_for(inst);
    tracker.touch(); // someone is watching: keep player tracking on
    json_response(
        &json!({"players": tracker.snapshot()}),
        StatusCode::OK,
        "no-store",
    )
}

/// Tier list and records of the server's players (see leaderboard.rs for what it contains).
async fn stats_json(inst: &Instance) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let i = inst.clone();
    let data = tokio::task::spawn_blocking(move || leaderboard::build(&i))
        .await
        .unwrap_or(Value::Null);
    json_response(&data, StatusCode::OK, "no-cache")
}

/// Which player this visitor probably is (by the address of their last login); never an address.
fn me_json(inst: &Instance, address: &str) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    json_response(
        &json!({"name": leaderboard::me(inst, Some(address))}),
        StatusCode::OK,
        "no-store",
    )
}

// ---------- waypoints ----------

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn visitor(inst: &Instance, headers: &HeaderMap, address: &str) -> Visitor {
    Visitor::new(
        inst,
        header_str(headers, "x-map-player").as_deref(),
        header_str(headers, "x-map-key").as_deref(),
        Some(address),
    )
}

/// Writes only from the map page: browsers send Origin with them; a cross-site page differs.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = header_str(headers, "origin") else {
        return true; // not a browser (curl & co. have no address-based power a page could abuse)
    };
    let host = header_str(headers, "host").unwrap_or_default();
    origin.split_once("://").map(|(_, h)| h).unwrap_or(&origin) == host
}

async fn waypoints_json(inst: &Instance, headers: &HeaderMap, address: &str) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    let (i, h, a) = (inst.clone(), headers.clone(), address.to_string());
    let data =
        tokio::task::spawn_blocking(move || waypoints::listing(&i, Some(&visitor(&i, &h, &a))))
            .await
            .unwrap_or(Value::Null);
    json_response(&data, StatusCode::OK, "no-store")
}

fn detail(status: StatusCode, text: &str) -> Response {
    json_response(&json!({"detail": text}), status, "no-store")
}

async fn body_of(req: Request) -> Result<Map<String, Value>, String> {
    let ctype = header_str(req.headers(), "content-type").unwrap_or_default();
    if ctype.split(';').next().unwrap_or("").trim() != "application/json" {
        return Err("Expected JSON".into());
    }
    let raw = axum::body::to_bytes(req.into_body(), BODY_MAX)
        .await
        .map_err(|_| "Request too large".to_string())?;
    let data: Value = serde_json::from_slice(&raw).map_err(|_| "Bad JSON".to_string())?;
    match data {
        Value::Object(m) => Ok(m),
        _ => Err("Bad request".into()),
    }
}

/// POST waypoints (create), PUT / DELETE waypoints/<id>.
async fn waypoint_write(
    inst: &Instance,
    wid: Option<String>,
    req: Request,
    address: &str,
) -> Response {
    if !map_enabled(inst) {
        return off();
    }
    if !same_origin(req.headers()) {
        return detail(StatusCode::FORBIDDEN, "Not allowed from another site");
    }
    let v = visitor(inst, req.headers(), address);
    let method = req.method().clone();
    let inst = inst.clone();
    if method == Method::DELETE {
        let wid = wid.unwrap_or_default();
        let r = tokio::task::spawn_blocking(move || waypoints::delete(&inst, &wid, Some(&v))).await;
        return match r {
            Ok(Ok(())) => json_response(&json!({"ok": true}), StatusCode::OK, "no-store"),
            Ok(Err(e)) => wp_error(e),
            Err(_) => detail(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"),
        };
    }
    let body = match body_of(req).await {
        Ok(b) => b,
        Err(e) => return detail(StatusCode::BAD_REQUEST, &e),
    };
    let r = tokio::task::spawn_blocking(move || match wid {
        None => waypoints::create(&inst, &body, Some(&v), None)
            .map(|w| (StatusCode::CREATED, w))
            .map_err(WpError::from),
        Some(wid) => waypoints::update(&inst, &wid, &body, Some(&v)).map(|w| (StatusCode::OK, w)),
    })
    .await;
    match r {
        Ok(Ok((status, w))) => json_response(&w, status, "no-store"),
        Ok(Err(e)) => wp_error(e),
        Err(_) => detail(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"),
    }
}

fn wp_error(e: WpError) -> Response {
    match e {
        WpError::NotFound => detail(StatusCode::NOT_FOUND, "No such waypoint"),
        WpError::Forbidden(t) => detail(StatusCode::FORBIDDEN, &t),
        WpError::Invalid(t) => detail(StatusCode::BAD_REQUEST, &t),
    }
}

/// The map server's router: one handler that dispatches by path.
pub fn router() -> axum::Router {
    axum::Router::new().fallback(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        let mut h = HeaderMap::new();
        assert!(same_origin(&h));
        h.insert("host", HeaderValue::from_static("192.168.1.2:47319"));
        h.insert(
            "origin",
            HeaderValue::from_static("http://192.168.1.2:47319"),
        );
        assert!(same_origin(&h));
        h.insert("origin", HeaderValue::from_static("https://evil.example"));
        assert!(!same_origin(&h));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent("a%20b"), "a b");
        assert_eq!(percent("tiles/a+b.png"), "tiles/a+b.png");
    }
}
