//! The admin panel's web app: REST actions + WebSocket live feed + the frontend files.
//!
//! Everything that concerns one server lives under `/api/servers/{sid}/...`; login, version lists,
//! downloaded jars and the server list itself are global.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use regex::Regex;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;

use crate::access::{self, Kind};
use crate::config::{self, cfg};
use crate::error::Error;
use crate::gamerules::{self, Rule};
use crate::instances;
use crate::server::{self, HUB, ServerManager, State};
use crate::sessions::{self, Check};
use crate::util::{hex_token, lock};
use crate::{
    addons, backup, datapacks, java, livemap, players, properties, servericon, statics, versions,
    waypoints, world,
};

const COOKIE: &str = "panel_session";
pub const HISTORY_INTERVAL: u64 = 30;

// ---------- errors ----------

pub struct ApiError(StatusCode, String);

impl ApiError {
    pub fn new(status: StatusCode, text: impl Into<String>) -> ApiError {
        ApiError(status, text.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"detail": self.1}))).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let status = match &e {
            Error::Busy(_) | Error::Exists(_) => StatusCode::CONFLICT,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::InUse(m) => {
                return ApiError::new(
                    StatusCode::CONFLICT,
                    format!("File in use, stop the server first ({m})"),
                );
            }
            Error::Remote(_) | Error::Status(..) => StatusCode::BAD_GATEWAY,
            Error::Invalid(_) | Error::Io(_) => StatusCode::BAD_REQUEST,
        };
        ApiError(status, e.to_string())
    }
}

type ApiResult = Result<Response, ApiError>;

fn ok() -> ApiResult {
    Ok(Json(json!({"ok": true})).into_response())
}

fn json_ok(v: Value) -> ApiResult {
    Ok(Json(v).into_response())
}

/// Request body as JSON (422 like FastAPI when it doesn't fit).
fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("Invalid request: {e}"),
        )
    })
}

/// Run blocking work (files, network, child processes) off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
}

/// Path parameter -> manager of that server (404 if unknown).
fn server(sid: &str) -> Result<Arc<ServerManager>, ApiError> {
    instances::validate_id(sid)
        .ok()
        .and_then(server::get)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Unknown server"))
}

// ---------- auth (sessions survive restarts, see sessions.rs) ----------

fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k == COOKIE).then(|| v.trim_matches('"').to_string())
        })
}

fn set_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; HttpOnly; Max-Age={}; Path=/; SameSite=strict",
        sessions::TTL
    ))
    .unwrap()
}

async fn require_auth(req: Request, next: Next) -> Response {
    let token = cookie(req.headers());
    let t = token.clone();
    let result = tokio::task::spawn_blocking(move || sessions::check(t.as_deref()))
        .await
        .unwrap_or(Check::Invalid);
    if result == Check::Invalid {
        return ApiError::new(StatusCode::UNAUTHORIZED, "Not logged in").into_response();
    }
    let mut response = next.run(req).await;
    if result == Check::Renewed {
        // sliding expiry: keep the browser cookie alive too
        response
            .headers_mut()
            .append(header::SET_COOKIE, set_cookie(&token.unwrap_or_default()));
    }
    response
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(body: Bytes) -> ApiResult {
    let b: LoginBody = parse(&body)?;
    if !crate::util::ct_eq(b.password.as_bytes(), cfg().password.as_bytes()) {
        tokio::time::sleep(Duration::from_secs(1)).await; // slow down guessing
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong password"));
    }
    let token = blocking(sessions::create).await?;
    let mut r = Json(json!({"ok": true})).into_response();
    r.headers_mut()
        .append(header::SET_COOKIE, set_cookie(&token));
    Ok(r)
}

async fn logout(headers: HeaderMap) -> ApiResult {
    let token = cookie(&headers);
    blocking(move || sessions::remove(token.as_deref())).await?;
    let mut r = Json(json!({"ok": true})).into_response();
    r.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_static("panel_session=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=lax"),
    );
    Ok(r)
}

// ---------- global routes ----------

#[derive(Deserialize)]
struct VersionsQuery {
    #[serde(default = "vanilla")]
    flavor: String,
    #[serde(default)]
    snapshots: Option<String>,
}

fn vanilla() -> String {
    "vanilla".into()
}

fn truthy(v: &Option<String>) -> bool {
    v.as_deref()
        .is_some_and(|s| matches!(s.to_lowercase().as_str(), "true" | "1" | "yes" | "on"))
}

async fn list_versions(Query(q): Query<VersionsQuery>) -> ApiResult {
    versions::validate_flavor(&q.flavor)?;
    let snapshots = truthy(&q.snapshots);
    let flavor = q.flavor.clone();
    match blocking(move || versions::list_versions(&flavor, snapshots)).await? {
        Ok(v) => json_ok(v),
        Err(e) => {
            let source = match q.flavor.as_str() {
                "vanilla" => "Mojang",
                "paper" => "PaperMC",
                _ => "Fabric",
            };
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("Could not load version list from {source}: {e}"),
            ))
        }
    }
}

async fn delete_version(Path((flavor, version)): Path<(String, String)>) -> ApiResult {
    for m in server::all() {
        let (rf, rv) = m.running();
        if m.is_alive()
            && rf.as_deref() == Some(flavor.as_str())
            && rv.as_deref() == Some(version.as_str())
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "This server version is currently running",
            ));
        }
        if m.task().as_deref() == Some("download") {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "A download is in progress",
            ));
        }
    }
    let dirs: Vec<PathBuf> = server::all().iter().map(|m| m.inst.server_dir()).collect();
    blocking(move || versions::delete(&flavor, &version, &dirs)).await??;
    ok()
}

// ---------- server list ----------

fn server_summary(id: &str, name: &str) -> Value {
    let Some(m) = server::get(id) else {
        return Value::Null;
    };
    let s = config::load_settings(&m.inst);
    json!({
        "id": id, "name": name, "state": m.state().as_str(), "task": m.task(), "players": m.players().len(),
        "port": instances::game_port(&m.inst), "flavor": s.flavor, "version": s.version, "has_map": true,
    })
}

async fn list_servers() -> ApiResult {
    json_ok(
        blocking(|| {
            Value::Array(
                instances::all_entries()
                    .iter()
                    .map(|e| server_summary(&e.id, &e.name))
                    .collect(),
            )
        })
        .await?,
    )
}

#[derive(Deserialize)]
struct NewServerBody {
    name: String,
    #[serde(default = "paper")]
    flavor: String,
    #[serde(default)]
    version: String,
    #[serde(default = "one_g")]
    min_ram: String,
    #[serde(default = "two_g")]
    max_ram: String,
}

fn paper() -> String {
    "paper".into()
}
fn one_g() -> String {
    "1G".into()
}
fn two_g() -> String {
    "2G".into()
}

fn create_server_now(b: NewServerBody) -> crate::error::Result<Value> {
    let flavor = versions::validate_flavor(&b.flavor)?.to_string();
    let version = if b.version.is_empty() {
        String::new()
    } else {
        config::validate_version(&b.version)?
    };
    let (min_ram, max_ram) = (
        config::validate_ram(&b.min_ram)?,
        config::validate_ram(&b.max_ram)?,
    );
    if config::ram_mb(&min_ram) > config::ram_mb(&max_ram) {
        return crate::invalid!("Min RAM must not exceed max RAM");
    }
    instances::validate_name(&b.name)?;
    let port = instances::free_port(None)?;
    let inst = instances::create(&b.name)?;
    access::set_property(&inst, "server-port", &port.to_string())?;
    let s = config::Settings {
        flavor,
        version,
        min_ram,
        max_ram,
        map_enabled: false,
        ..Default::default()
    };
    config::save_settings(&s, &inst)?;
    let m = server::add(inst.clone());
    m.info(&format!("Server created (game port {port})."));
    Ok(server_summary(&inst.id, &instances::name_of(&inst)))
}

async fn create_server(body: Bytes) -> ApiResult {
    let b: NewServerBody = parse(&body)?;
    match blocking(move || create_server_now(b)).await? {
        Ok(v) => json_ok(v),
        Err(e) => Err(ApiError::new(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename_server(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: RenameBody = parse(&body)?;
    instances::rename(&m.inst, &b.name)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    json_ok(server_summary(&m.inst.id, &instances::name_of(&m.inst)))
}

#[derive(Deserialize)]
struct PortBody {
    port: u16,
}

/// Change the game port (server-port); only while the server is stopped.
async fn set_port(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: PortBody = parse(&body)?;
    if m.is_alive() || m.task().is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, "Stop the server first"));
    }
    instances::set_port(&m.inst, b.port)?;
    m.info(&format!("Game port changed to {}.", b.port));
    json_ok(server_summary(&m.inst.id, &instances::name_of(&m.inst)))
}

async fn delete_server(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    if m.inst.is_main() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "The first server can't be deleted",
        ));
    }
    if m.is_alive() || m.task().is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, "Stop the server first"));
    }
    server::remove(&m.inst.id);
    let inst = m.inst.clone();
    blocking(move || instances::remove(&inst)).await??;
    ok()
}

// ---------- per-server routes ----------

fn state_of(m: &ServerManager) -> Value {
    let s = config::load_settings(&m.inst);
    // World not started through the panel yet (e.g. imported): ask level.dat
    let world_version = if s.world_version.is_empty() {
        world::saved_version(&m.inst).unwrap_or_default()
    } else {
        s.world_version.clone()
    };
    let opt = |v: &str| if v.is_empty() { Value::Null } else { json!(v) };
    let c = cfg();
    json!({
        "server": {"id": m.inst.id, "name": instances::name_of(&m.inst), "port": instances::game_port(&m.inst), "has_map": true},
        "status": m.status(),
        "settings": {"flavor": s.flavor, "version": s.version, "min_ram": s.min_ram, "max_ram": s.max_ram,
                     "map_enabled": s.map_enabled},
        "world": {
            "version": opt(&world_version),
            "flavor": opt(&s.world_flavor),
            "rank": if world_version.is_empty() { None } else { versions::rank(&world_version) },
        },
        "installed": versions::installed(),
        "java": java::runtimes(),
        "log": m.log_lines(),
        "chat": m.chat(),
        "backups": backup::list_backups(&m.inst),
        "backup_keep": c.backup_keep,
        "backup_interval": c.backup_interval_hours,
    })
}

async fn state(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    // May hit the network (version ranks) and spawn `java -version`: keep it off the event loop
    json_ok(blocking(move || state_of(&m)).await?)
}

async fn start(Path(sid): Path<String>) -> ApiResult {
    server(&sid)?.start()?;
    ok()
}

#[derive(Deserialize, Default)]
struct StopBody {
    #[serde(default)]
    countdown: i64,
}

fn stop_body(body: &Bytes) -> Result<u64, ApiError> {
    let b: StopBody = if body.is_empty() {
        StopBody::default()
    } else {
        parse(body)?
    };
    Ok(b.countdown.clamp(0, 300) as u64)
}

async fn stop(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    server(&sid)?.stop(stop_body(&body)?)?;
    ok()
}

async fn restart(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    server(&sid)?.restart(stop_body(&body)?)?;
    ok()
}

async fn kill(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    blocking(move || m.kill()).await?;
    ok()
}

async fn cancel(Path(sid): Path<String>) -> ApiResult {
    server(&sid)?.cancel_countdown();
    ok()
}

#[derive(Deserialize)]
struct CommandBody {
    command: String,
}

async fn command(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: CommandBody = parse(&body)?;
    let cmd = b.command.trim().trim_start_matches('/');
    if cmd.is_empty() || cmd.contains(['\n', '\r']) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Invalid command"));
    }
    m.send_command(cmd)?;
    ok()
}

async fn eula(Path(sid): Path<String>) -> ApiResult {
    server(&sid)?.accept_eula()?;
    ok()
}

#[derive(Deserialize)]
struct VersionBody {
    #[serde(default = "vanilla")]
    flavor: String,
    version: String,
}

async fn install(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: VersionBody = parse(&body)?;
    versions::jar_path(&b.flavor, &b.version)?; // validates both
    m.install(&b.flavor, &b.version)?;
    ok()
}

/// Newest Paper build / Fabric loader for a downloaded server.
async fn update_jar(Path((sid, flavor, version)): Path<(String, String, String)>) -> ApiResult {
    let m = server(&sid)?;
    if flavor != "paper" && flavor != "fabric" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Only Paper and Fabric servers have updates",
        ));
    }
    if !versions::jar_path(&flavor, &version)?.exists() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!(
                "{} {version} is not downloaded",
                crate::util::capitalize(&flavor)
            ),
        ));
    }
    m.update_jar(&flavor, &version)?;
    ok()
}

#[derive(Deserialize)]
struct SettingsBody {
    map_enabled: Option<bool>,
    flavor: Option<String>,
    version: Option<String>,
    min_ram: Option<String>,
    max_ram: Option<String>,
}

async fn update_settings(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: SettingsBody = parse(&body)?;
    let mut s = config::load_settings(&m.inst);
    if let Some(v) = &b.min_ram {
        s.min_ram = config::validate_ram(v)?;
    }
    if let Some(v) = &b.max_ram {
        s.max_ram = config::validate_ram(v)?;
    }
    if config::ram_mb(&s.min_ram) > config::ram_mb(&s.max_ram) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Min RAM must not exceed max RAM",
        ));
    }
    let new_flavor = b.flavor.clone().unwrap_or_else(|| s.flavor.clone());
    let new_version = b.version.clone().unwrap_or_else(|| s.version.clone());
    if (new_flavor.as_str(), new_version.as_str()) != (s.flavor.as_str(), s.version.as_str()) {
        if m.is_alive() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "Stop the server before changing the version",
            ));
        }
        let new_flavor = versions::validate_flavor(&new_flavor)?.to_string();
        s.version = if new_version.is_empty() {
            String::new()
        } else {
            config::validate_version(&new_version)?
        };
        if new_flavor != s.flavor {
            // Another server type changes the world layout / drops mod blocks: keep a way back
            let mm = m.clone();
            let suffix = format!("before-{new_flavor}");
            match blocking(move || mm.backup_before(&suffix)).await? {
                Ok(()) => {}
                Err(Error::Busy(e)) => return Err(ApiError::new(StatusCode::CONFLICT, e)),
                Err(e) => {
                    return Err(ApiError::new(
                        StatusCode::BAD_REQUEST,
                        format!("Backup before switching failed: {e}"),
                    ));
                }
            }
            s.flavor = new_flavor;
        }
    }
    if let Some(on) = b.map_enabled {
        s.map_enabled = on;
        m.map_enabled.store(on, std::sync::atomic::Ordering::SeqCst); // public map page follows immediately
    }
    config::save_settings(&s, &m.inst).map_err(Error::from)?;
    m.emit_status();
    ok()
}

// ---------- whitelist & operators ----------

#[derive(Deserialize)]
struct AccessBody {
    action: String,
    name: String,
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

/// true = use server commands, false = edit files; refuse in between.
fn online(m: &ServerManager) -> Result<bool, ApiError> {
    if !m.is_alive() {
        return Ok(false);
    }
    match m.state() {
        State::Running => Ok(true),
        other => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("Server is {}, try again in a moment", other.as_str()),
        )),
    }
}

async fn get_access(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    json_ok(blocking(move || access::state(&m.inst)).await?)
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

async fn change(sid: String, kind: Kind, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: AccessBody = parse(&body)?;
    if b.action != "add" && b.action != "remove" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "action must be add or remove",
        ));
    }
    let add = b.action == "add";
    let name = access::validate_name(&b.name)?;
    let reason = access::validate_reason(&b.reason)?;
    if online(&m)? {
        let cmd = match (kind, add) {
            (Kind::Whitelist, true) => "whitelist add",
            (Kind::Whitelist, false) => "whitelist remove",
            (Kind::Ops, true) => "op",
            (Kind::Ops, false) => "deop",
            (Kind::Bans, true) => "ban",
            (Kind::Bans, false) => "pardon",
        };
        let mut line = format!("{cmd} {name}");
        if kind == Kind::Bans && add && !reason.is_empty() {
            line.push(' ');
            line.push_str(&reason);
        }
        let before = access::mtime(&m.inst, kind.file());
        m.send_command(&line)?;
        // The server answers in the console (e.g. unknown player); wait for its file update
        let inst = m.inst.clone();
        let st = blocking(move || {
            access::wait_for_change(&inst, kind.file(), before, Duration::from_secs(3));
            access::state(&inst)
        })
        .await?;
        return json_ok(merge(
            json!({"ok": true, "message": format!("Sent: /{line}")}),
            st,
        ));
    }
    let inst = m.inst.clone();
    let (n, r) = (name.clone(), reason.clone());
    let msg = match blocking(move || access::offline_change(&inst, kind, add, &n, &r)).await? {
        Ok(msg) => msg,
        Err(Error::NotFound(e)) => return Err(ApiError::new(StatusCode::NOT_FOUND, e)),
        Err(e @ (Error::Remote(_) | Error::Status(..))) => {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("Could not reach Mojang to look up {name}: {e}"),
            ));
        }
        Err(e) => return Err(e.into()),
    };
    m.info(&msg);
    let inst = m.inst.clone();
    let st = blocking(move || access::state(&inst)).await?;
    json_ok(merge(json!({"ok": true, "message": msg}), st))
}

async fn change_whitelist(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    change(sid, Kind::Whitelist, body).await
}

async fn change_ops(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    change(sid, Kind::Ops, body).await
}

async fn change_bans(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    change(sid, Kind::Bans, body).await
}

async fn set_whitelist_enabled(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: EnabledBody = parse(&body)?;
    if online(&m)? {
        m.send_command(&format!(
            "whitelist {}",
            if b.enabled { "on" } else { "off" }
        ))?;
        tokio::time::sleep(Duration::from_millis(500)).await; // server writes server.properties
    } else {
        access::set_property(
            &m.inst,
            "white-list",
            if b.enabled { "true" } else { "false" },
        )
        .map_err(Error::from)?;
        m.info(&format!(
            "Whitelist {} (applies on next start)",
            if b.enabled { "enabled" } else { "disabled" }
        ));
    }
    let inst = m.inst.clone();
    json_ok(merge(
        json!({"ok": true}),
        blocking(move || access::state(&inst)).await?,
    ))
}

// ---------- player profiles & actions ----------

async fn player_profile(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let name = access::validate_name(&name)?;
    let online = m
        .players()
        .iter()
        .any(|n| n.to_lowercase() == name.to_lowercase());
    let inst = m.inst.clone();
    let mut p = blocking(move || players::profile(&inst, &name)).await?;
    p.insert("online".into(), json!(online));
    json_ok(Value::Object(p))
}

/// The player's skin PNG (from Mojang, cached); the browser cuts out the face. 404 = default head.
async fn player_skin(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let name = access::validate_name(&name)?;
    let inst = m.inst.clone();
    let path = blocking(move || players::skin(&inst, &name)).await?;
    let Some(data) = path.and_then(|p| std::fs::read(p).ok()) else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "No skin"));
    };
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        data,
    )
        .into_response())
}

#[derive(Deserialize)]
struct PlayerActionBody {
    action: String,
    #[serde(default)]
    value: String,
}

/// Console commands for one player; built here, so nothing but a validated name goes in.
async fn player_action(Path((sid, name)): Path<(String, String)>, body: Bytes) -> ApiResult {
    static DIM_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[a-z0-9_.\-]+:[a-z0-9_./\-]+$").unwrap());
    let m = server(&sid)?;
    let b: PlayerActionBody = parse(&body)?;
    let name = access::validate_name(&name)?;
    let reason = if b.action == "kick" {
        access::validate_reason(&b.value)?
    } else {
        String::new()
    };
    if m.state() != State::Running {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "The server must be running",
        ));
    }
    let cmds: Vec<String> = match b.action.as_str() {
        "gamemode" => {
            if !players::GAMEMODES.contains(&b.value.as_str()) {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "Unknown game mode"));
            }
            vec![format!("gamemode {} {name}", b.value)]
        }
        "spawn" => {
            let inst = m.inst.clone();
            let Some((dim, [x, y, z])) = blocking(move || players::world_spawn(&inst)).await?
            else {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "World spawn not found in level.dat",
                ));
            };
            if !DIM_RE.is_match(&dim) {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "Unexpected spawn dimension",
                ));
            }
            vec![format!(
                "execute in {dim} run tp {name} {} {y} {}",
                x as f64 + 0.5,
                z as f64 + 0.5
            )]
        }
        // Instant health 10 fills any health bar, saturation fills hunger
        "heal" => vec![
            format!("effect give {name} minecraft:instant_health 1 10 true"),
            format!("effect give {name} minecraft:saturation 1 10 true"),
        ],
        "clear" => vec![format!("clear {name}")],
        "kick" => vec![format!(
            "kick {name}{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(" {reason}")
            }
        )],
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "Unknown action")),
    };
    for cmd in &cmds {
        m.send_command(cmd)?;
    }
    json_ok(json!({"ok": true, "message": format!("Sent: /{}", cmds[0])}))
}

// ---------- backups & world ----------

async fn create_backup(Path(sid): Path<String>) -> ApiResult {
    server(&sid)?.make_backup()?;
    ok()
}

async fn get_backups(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    json_ok(blocking(move || backup::list_backups(&m.inst)).await?)
}

/// Stream a raw request body to a temporary file in DATA_DIR (the caller deletes it).
async fn receive(body: Body, limit: u64) -> Result<PathBuf, ApiError> {
    let tmp = cfg().data_dir.join(format!(".upload-{}", hex_token(8)));
    let result = async {
        let mut f = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| ApiError::from(Error::from(e)))?;
        let mut stream = body.into_data_stream();
        let mut size: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                ApiError::new(StatusCode::BAD_REQUEST, format!("Upload failed: {e}"))
            })?;
            size += chunk.len() as u64;
            if size > limit {
                return Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("File too large (max {} MB)", limit >> 20),
                ));
            }
            f.write_all(&chunk)
                .await
                .map_err(|e| ApiError::from(Error::from(e)))?;
        }
        f.flush()
            .await
            .map_err(|e| ApiError::from(Error::from(e)))?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(tmp)
}

/// A world as .zip (server world or single-player save) -> stored as a backup, then restorable.
async fn upload_world(Path(sid): Path<String>, body: Body) -> ApiResult {
    let m = server(&sid)?;
    let tmp = receive(body, cfg().world_upload_max).await?;
    let inst = m.inst.clone();
    let t = tmp.clone();
    let result = blocking(move || backup::add_upload(&inst, &t)).await?;
    let _ = std::fs::remove_file(&tmp);
    let path = result.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    m.info(&format!("World uploaded as {name}: restore it to use it."));
    let inst = m.inst.clone();
    json_ok(json!({"name": name, "backups": blocking(move || backup::list_backups(&inst)).await?}))
}

async fn download_backup(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let path = backup::backup_file(&m.inst, &name)
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "Backup not found"))?;
    statics::download(&path, &name)
        .await
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Backup not found"))
}

async fn restore_backup(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    match m.restore(&name) {
        Err(Error::NotFound(_)) => Err(ApiError::new(StatusCode::NOT_FOUND, "Backup not found")),
        other => other.map_err(ApiError::from).and_then(|_| ok()),
    }
}

async fn render_map(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    if livemap::find_binary().is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Map renderer 'mapgen' not found (build it: cargo build --release in mapgen/)",
        ));
    }
    m.render_map()?;
    ok()
}

// What the 3D live map shows below the surface (cave reach); see livemap.rs
async fn get_map_visibility() -> ApiResult {
    json_ok(livemap::load_visibility())
}

async fn put_map_visibility(body: Bytes) -> ApiResult {
    let b: Map<String, Value> = parse(&body)?;
    let v = livemap::save_visibility(&b)?;
    // re-render now (mapgen sees the changed settings and redoes the 3D stacks); skipped while
    // busy, then the next scheduled render does it
    if livemap::find_binary().is_some() {
        for m in server::all() {
            if m.map_enabled.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::spawn(move || {
                    livemap::renderer_for(&m.inst).render(&|t| m.info(t), false, false);
                });
            }
        }
    }
    json_ok(v)
}

// Waypoints of the live map (waypoints.rs): the panel may change all of them
async fn list_waypoints(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    json_ok(blocking(move || waypoints::listing(&m.inst, None)).await?)
}

async fn create_waypoint(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: Map<String, Value> = parse(&body)?;
    let inst = m.inst.clone();
    let r = blocking(move || {
        let owner = b
            .get("owner")
            .and_then(Value::as_str)
            .filter(|o| !o.is_empty())
            .map(str::to_string);
        waypoints::create(&inst, &b, None, owner.as_deref())
    })
    .await?;
    json_ok(r.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?)
}

fn wp_error(e: waypoints::WpError) -> ApiError {
    match e {
        waypoints::WpError::NotFound => ApiError::new(StatusCode::NOT_FOUND, "No such waypoint"),
        waypoints::WpError::Forbidden(t) => ApiError::new(StatusCode::FORBIDDEN, t),
        waypoints::WpError::Invalid(t) => ApiError::new(StatusCode::BAD_REQUEST, t),
    }
}

async fn update_waypoint(Path((sid, wid)): Path<(String, String)>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: Map<String, Value> = parse(&body)?;
    let inst = m.inst.clone();
    json_ok(
        blocking(move || waypoints::update(&inst, &wid, &b, None))
            .await?
            .map_err(wp_error)?,
    )
}

async fn delete_waypoint(Path((sid, wid)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let inst = m.inst.clone();
    blocking(move || waypoints::delete(&inst, &wid, None))
        .await?
        .map_err(wp_error)?;
    ok()
}

#[derive(Deserialize)]
struct NewWorldBody {
    #[serde(default)]
    seed: String,
    #[serde(default)]
    datapacks: Vec<String>, // installed into the new world before it is generated
}

async fn new_world(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: NewWorldBody = parse(&body)?;
    m.new_world(&b.seed, &b.datapacks)?;
    ok()
}

// ---------- data packs ----------

/// Send a /datapack or /reload command, then save until level.dat shows the result.
async fn apply_online(
    m: &Arc<ServerManager>,
    cmd: &str,
    expect: impl Fn(&datapacks::PackState) -> bool,
) -> Result<bool, ApiError> {
    if let Some(task) = m.task() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("Another action is running: {task}"),
        ));
    }
    m.send_command(cmd)?;
    for _ in 0..3 {
        // the reload behind the command runs asynchronously
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mm = m.clone();
        let st = blocking(move || {
            mm.save_and_wait(Duration::from_secs(30));
            datapacks::state(&mm.inst)
        })
        .await?;
        if let Ok(Some(st)) = st {
            if expect(&st) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Copy library -> world; while running, /reload so Minecraft picks it up.
async fn install_pack(m: &Arc<ServerManager>, name: &str) -> Result<(), ApiError> {
    let is_online = online(m)?;
    let (inst, n) = (m.inst.clone(), name.to_string());
    blocking(move || datapacks::install(&inst, &n)).await??;
    if is_online {
        let pid = datapacks::pack_id(name);
        // /reload discovers new packs and enables them (a pack disabled before stays disabled)
        let ok = apply_online(m, "minecraft:reload", |st| {
            st.0.contains(&pid) || st.1.contains(&pid)
        })
        .await?;
        m.info(&format!(
            "Data pack {name} installed{}",
            if ok {
                " and loaded."
            } else {
                ": see the console for errors."
            }
        ));
    } else {
        m.info(&format!(
            "Data pack {name} installed in the world, loaded on next start."
        ));
    }
    Ok(())
}

async fn packs(m: &Arc<ServerManager>) -> ApiResult {
    let inst = m.inst.clone();
    json_ok(blocking(move || datapacks::list_packs(&inst)).await?)
}

async fn list_datapacks(Path(sid): Path<String>) -> ApiResult {
    packs(&server(&sid)?).await
}

/// pack.png of a data pack (the list says `icon: true` when there is one).
async fn datapack_icon(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let inst = m.inst.clone();
    let data = blocking(move || datapacks::icon(&inst, &name)).await??;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "private, max-age=300"),
        ],
        data,
    )
        .into_response())
}

#[derive(Deserialize)]
struct NameQuery {
    name: String,
}

/// Raw .zip body -> library, then installed into this server's world.
async fn upload_datapack(
    Path(sid): Path<String>,
    Query(q): Query<NameQuery>,
    body: Body,
) -> ApiResult {
    let m = server(&sid)?;
    online(&m)?; // refuse early while starting/stopping
    datapacks::validate_name(&q.name)?;
    let tmp = receive(body, datapacks::MAX_UPLOAD).await?;
    let (n, t) = (q.name.clone(), tmp.clone());
    let result = blocking(move || datapacks::add_upload(&n, &t)).await?;
    let _ = std::fs::remove_file(&tmp);
    result?;
    m.info(&format!("Data pack {} added to the library.", q.name));
    install_pack(&m, &q.name).await?;
    packs(&m).await
}

async fn install_datapack(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    install_pack(&m, &name).await?;
    packs(&m).await
}

async fn adopt_datapack(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let (inst, n) = (m.inst.clone(), name.clone());
    blocking(move || datapacks::adopt(&inst, &n)).await??;
    m.info(&format!("Data pack {name} copied to the library."));
    packs(&m).await
}

async fn toggle_datapack(Path((sid, name)): Path<(String, String)>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: EnabledBody = parse(&body)?;
    datapacks::path_of(&m.inst, &name)?;
    if online(&m)? {
        let pid = datapacks::pack_id(&name);
        let verb = if b.enabled { "enable" } else { "disable" };
        let enabled = b.enabled;
        let ok = apply_online(&m, &format!("minecraft:datapack {verb} \"{pid}\""), |st| {
            st.0.contains(&pid) == enabled
        })
        .await?;
        if !ok {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("The server did not {verb} {name}: see the console"),
            ));
        }
        m.info(&format!("Data pack {name} {verb}d."));
    } else {
        let (inst, n) = (m.inst.clone(), name.clone());
        let msg = blocking(move || datapacks::set_enabled_offline(&inst, &n, b.enabled)).await??;
        m.info(&msg);
    }
    packs(&m).await
}

async fn uninstall_datapack(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    if m.is_alive() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Stop the server before removing a data pack from the world (or disable it)",
        ));
    }
    let (inst, n) = (m.inst.clone(), name.clone());
    blocking(move || datapacks::uninstall(&inst, &n)).await??;
    m.info(&format!("Data pack {name} removed from the world."));
    packs(&m).await
}

async fn delete_datapack(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let n = name.clone();
    blocking(move || datapacks::delete_from_library(&n)).await??;
    m.info(&format!("Data pack {name} deleted from the library."));
    packs(&m).await
}

// ---------- mods (Fabric) & plugins (Paper) ----------

/// (kind, game version) of the server's selected type; 400 for Vanilla.
fn addon_kind(m: &ServerManager) -> Result<(&'static str, String), ApiError> {
    let s = config::load_settings(&m.inst);
    match addons::kind_of(&s.flavor) {
        Some(kind) => Ok((kind, s.version)),
        None => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Vanilla servers have no mods or plugins: switch to Paper or Fabric",
        )),
    }
}

fn addon_list(m: &ServerManager) -> Value {
    let s = config::load_settings(&m.inst);
    let kind = addons::kind_of(&s.flavor);
    json!({
        "kind": kind, "flavor": s.flavor, "version": s.version, "running": m.is_alive(),
        "items": kind.map(|k| addons::list_addons(&m.inst, k, &s.version)).unwrap_or_default(),
    })
}

fn addon_changed(m: &ServerManager, text: &str) {
    m.info(&format!(
        "{text}{}",
        if m.is_alive() {
            " Applies after a restart."
        } else {
            ""
        }
    ));
}

async fn addon_response(m: &Arc<ServerManager>) -> ApiResult {
    let mm = m.clone();
    json_ok(blocking(move || addon_list(&mm)).await?)
}

async fn list_addons(Path(sid): Path<String>) -> ApiResult {
    addon_response(&server(&sid)?).await
}

async fn upload_addon(
    Path(sid): Path<String>,
    Query(q): Query<NameQuery>,
    body: Body,
) -> ApiResult {
    let m = server(&sid)?;
    let (kind, _) = addon_kind(&m)?;
    addons::validate_file(&q.name)?;
    let tmp = receive(body, addons::MAX_UPLOAD).await?;
    let (inst, n, t) = (m.inst.clone(), q.name.clone(), tmp.clone());
    let result = blocking(move || addons::add_upload(&inst, kind, &n, &t)).await?;
    let _ = std::fs::remove_file(&tmp);
    result?;
    addon_changed(&m, &format!("{} added to {kind}.", q.name));
    addon_response(&m).await
}

async fn toggle_addon(Path((sid, name)): Path<(String, String)>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let (kind, _) = addon_kind(&m)?;
    let b: EnabledBody = parse(&body)?;
    let inst = m.inst.clone();
    let new = blocking(move || addons::set_enabled(&inst, kind, &name, b.enabled)).await??;
    let base = new.strip_suffix(addons::DISABLED).unwrap_or(&new);
    addon_changed(
        &m,
        &format!("{base} {}.", if b.enabled { "enabled" } else { "disabled" }),
    );
    addon_response(&m).await
}

async fn delete_addon(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let (kind, _) = addon_kind(&m)?;
    let (inst, n) = (m.inst.clone(), name.clone());
    blocking(move || addons::delete(&inst, kind, &n)).await??;
    addon_changed(&m, &format!("{name} deleted."));
    addon_response(&m).await
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: i64,
}

async fn search_addons(Path(sid): Path<String>, Query(q): Query<SearchQuery>) -> ApiResult {
    let m = server(&sid)?;
    let (kind, version) = addon_kind(&m)?;
    json_ok(blocking(move || addons::search(kind, &version, &q.q, q.offset)).await??)
}

#[derive(Deserialize)]
struct AddonInstallBody {
    project_id: String,
}

async fn install_addon(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let (kind, version) = addon_kind(&m)?;
    if version.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Choose a Minecraft version first (Server page)",
        ));
    }
    let b: AddonInstallBody = parse(&body)?;
    let mm = m.clone();
    let names =
        blocking(move || addons::install(&mm.inst, kind, &version, &b.project_id, &|t| mm.info(t)))
            .await??;
    if !names.is_empty() {
        addon_changed(
            &m,
            &format!("Installed from Modrinth: {}.", names.join(", ")),
        );
    }
    let mm = m.clone();
    let list = blocking(move || addon_list(&mm)).await?;
    json_ok(merge(list, json!({"installed": names})))
}

async fn addon_updates(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    let (kind, version) = addon_kind(&m)?;
    json_ok(blocking(move || addons::check_updates(&m.inst, kind, &version)).await??)
}

async fn update_addon(Path((sid, name)): Path<(String, String)>) -> ApiResult {
    let m = server(&sid)?;
    let (kind, version) = addon_kind(&m)?;
    let (mm, n) = (m.clone(), name.clone());
    let new =
        blocking(move || addons::update(&mm.inst, kind, &version, &n, &|t| mm.info(t))).await??;
    addon_changed(&m, &format!("{name} updated to {new}."));
    addon_response(&m).await
}

// ---------- history, game rules ----------

/// Last hour of CPU/RAM/players/TPS (one point per 30 s while running).
async fn history(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    json_ok(json!({"interval": HISTORY_INTERVAL, "points": m.history()}))
}

async fn get_gamerules(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    let r = blocking(move || {
        let mut rules = gamerules::read(&m.inst);
        let mut live = false;
        if !rules.is_empty() && m.state() == State::Running && m.task().is_none() {
            // Ask the running server for every rule (answers are swallowed). Missing answers keep the file value.
            let cmds: Vec<String> = rules
                .iter()
                .map(|(n, _)| format!("gamerule {}", gamerules::command_name(n)))
                .collect();
            let lines = m.ask(
                &cmds,
                &gamerules::ANSWER_RE,
                rules.len(),
                Duration::from_secs(4),
            );
            let answers: HashMap<String, Rule> = gamerules::answers(&lines);
            for (name, value) in rules.iter_mut() {
                let found = answers
                    .get(name.as_str())
                    .or_else(|| answers.get(gamerules::command_name(name)));
                if let Some(v) = found.filter(|v| v.same_type(*value)) {
                    *value = *v;
                }
            }
            live = !answers.is_empty();
        }
        json!({"rules": gamerules::describe(&rules), "live": live, "world": !rules.is_empty()})
    })
    .await?;
    json_ok(r)
}

#[derive(Deserialize)]
struct GameruleBody {
    name: String,
    value: Value,
}

async fn set_gamerule(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let b: GameruleBody = parse(&body)?;
    let inst = m.inst.clone();
    let rules = blocking(move || gamerules::read(&inst)).await?;
    let value = gamerules::validate(&rules, &b.name, &b.value)?;
    let cmd_name = gamerules::command_name(&b.name).to_string();
    if online(&m)? {
        let cmd = format!("gamerule {cmd_name} {}", value.text());
        let mm = m.clone();
        let c = cmd.clone();
        let answers =
            blocking(move || mm.ask(&[c], &gamerules::ANSWER_RE, 1, Duration::from_secs(3)))
                .await?;
        m.info(&if answers.is_empty() {
            format!("Sent /{cmd} (no confirmation from the server: see the console).")
        } else {
            format!("Game rule {cmd_name} set to {}.", value.text())
        });
    } else {
        let (inst, n) = (m.inst.clone(), b.name.clone());
        blocking(move || gamerules::write_offline(&inst, &n, value))
            .await?
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
        m.info(&format!(
            "Game rule {cmd_name} set to {} (applies on next start).",
            value.text()
        ));
    }
    let v = match value {
        Rule::Bool(x) => json!(x),
        Rule::Int(x) => json!(x),
    };
    json_ok(json!({"ok": true, "name": b.name, "value": v}))
}

// ---------- server.properties ----------

async fn get_properties(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    json_ok(json!({"values": properties::get_all(&m.inst), "fields": properties::describe()}))
}

async fn set_properties(Path(sid): Path<String>, body: Bytes) -> ApiResult {
    let m = server(&sid)?;
    let values: Map<String, Value> = parse(&body)?;
    let result = properties::update(&m.inst, &values)?;
    m.info(&format!(
        "server.properties updated{}",
        if m.is_alive() {
            " (applies after a restart)"
        } else {
            ""
        }
    ));
    json_ok(json!({"values": result}))
}

// ---------- server list icon (server-icon.png) ----------

/// The icon Minecraft shows in the server list (the panel logo until one is uploaded).
async fn get_icon(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    let data = servericon::read(&m.inst);
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        data,
    )
        .into_response())
}

/// Raw PNG body, 64×64 (the page scales the user's image).
async fn put_icon(Path(sid): Path<String>, headers: HeaderMap, body: Body) -> ApiResult {
    let m = server(&sid)?;
    let too_big = || {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "The icon is too big (at most 256 KB)",
        )
    };
    let declared: u64 = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if declared > servericon::MAX_BYTES as u64 {
        return Err(too_big());
    }
    let data = axum::body::to_bytes(body, servericon::MAX_BYTES + 1)
        .await
        .map_err(|_| too_big())?;
    let inst = m.inst.clone();
    blocking(move || servericon::save(&inst, &data))
        .await?
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    m.info(&format!(
        "Server icon changed{}",
        if m.is_alive() {
            " (shows after a restart)"
        } else {
            ""
        }
    ));
    ok()
}

async fn reset_icon(Path(sid): Path<String>) -> ApiResult {
    let m = server(&sid)?;
    let inst = m.inst.clone();
    blocking(move || servericon::reset(&inst)).await??;
    m.info(&format!(
        "Server icon reset to the panel logo{}",
        if m.is_alive() {
            " (shows after a restart)"
        } else {
            ""
        }
    ));
    ok()
}

// ---------- health, WebSocket, frontend ----------

/// No login; for Docker's HEALTHCHECK.
async fn health() -> Json<Value> {
    Json(json!({"ok": true}))
}

async fn websocket(ws: WebSocketUpgrade, headers: HeaderMap) -> Response {
    let token = cookie(&headers);
    let valid =
        tokio::task::spawn_blocking(move || sessions::check(token.as_deref()) != Check::Invalid)
            .await
            .unwrap_or(false);
    ws.on_upgrade(move |socket| feed(socket, valid))
}

/// Cancelled when the panel shuts down, so open WebSockets don't hold up the graceful shutdown.
pub static SHUTDOWN: LazyLock<tokio_util::sync::CancellationToken> =
    LazyLock::new(Default::default);

async fn feed(mut socket: WebSocket, valid: bool) {
    if !valid {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 4401,
                reason: "Not logged in".into(),
            })))
            .await;
        return;
    }
    let mut rx = HUB.subscribe();
    loop {
        tokio::select! {
            _ = SHUTDOWN.cancelled() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            event = rx.recv() => match event {
                Ok(text) => {
                    if socket.send(Message::Text(text.as_ref().into())).await.is_err() {
                        break; // any send failure means the client is gone
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue, // slow client: events dropped
                Err(_) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}

static ASSET_HASHES: LazyLock<Mutex<HashMap<String, (u128, String)>>> =
    LazyLock::new(Default::default);

/// Short content hash, so a changed file gets a new URL (no stale browser cache).
pub fn asset_version(name: &str) -> String {
    let path = cfg().static_dir.join(name);
    let mtime = crate::util::mtime_ns(&path).unwrap_or(0);
    if let Some((m, h)) = lock(&ASSET_HASHES).get(name) {
        if *m == mtime {
            return h.clone();
        }
    }
    let h = hex::encode(Sha1::digest(std::fs::read(&path).unwrap_or_default()))[..10].to_string();
    lock(&ASSET_HASHES).insert(name.to_string(), (mtime, h.clone()));
    h
}

async fn index() -> Response {
    let html = std::fs::read_to_string(cfg().static_dir.join("index.html")).unwrap_or_default();
    let mut html = html;
    for name in ["app.js", "style.css", "icons.svg", "map/logo.svg"] {
        html = html.replace(
            &format!("/static/{name}"),
            &format!("/static/{name}?v={}", asset_version(name)),
        );
    }
    ([(header::CACHE_CONTROL, "no-cache")], Html(html)).into_response()
}

async fn static_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    // Browser may cache, but must check with the server (cheap 304) before reusing
    let Some(target) = statics::safe_join(&cfg().static_dir, &path) else {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "Not Found"}))).into_response();
    };
    statics::file(&target, &headers, "no-cache")
        .await
        .unwrap_or_else(|| {
            (StatusCode::NOT_FOUND, Json(json!({"detail": "Not Found"}))).into_response()
        })
}

pub fn router() -> Router {
    let srv = Router::new()
        .route("/state", get(state))
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/restart", post(restart))
        .route("/kill", post(kill))
        .route("/cancel", post(cancel))
        .route("/command", post(command))
        .route("/eula", post(eula))
        .route("/versions/install", post(install))
        .route("/versions/{flavor}/{version}/update", post(update_jar))
        .route("/settings", post(update_settings))
        .route("/access", get(get_access))
        .route("/access/whitelist", post(change_whitelist))
        .route("/access/ops", post(change_ops))
        .route("/access/bans", post(change_bans))
        .route("/access/whitelist/enabled", post(set_whitelist_enabled))
        .route("/players/{name}", get(player_profile))
        .route("/players/{name}/skin", get(player_skin))
        .route("/players/{name}/action", post(player_action))
        .route("/backups", post(create_backup).get(get_backups))
        .route("/backups/upload", post(upload_world))
        .route("/backups/{name}", get(download_backup))
        .route("/backups/{name}/restore", post(restore_backup))
        .route("/map/render", post(render_map))
        .route("/waypoints", get(list_waypoints).post(create_waypoint))
        .route(
            "/waypoints/{wid}",
            put(update_waypoint).delete(delete_waypoint),
        )
        .route("/world/new", post(new_world))
        .route("/datapacks", get(list_datapacks))
        .route("/datapacks/upload", post(upload_datapack))
        .route("/datapacks/{name}/icon", get(datapack_icon))
        .route("/datapacks/{name}/install", post(install_datapack))
        .route("/datapacks/{name}/adopt", post(adopt_datapack))
        .route("/datapacks/{name}/enabled", post(toggle_datapack))
        .route("/datapacks/{name}/world", delete(uninstall_datapack))
        .route("/datapacks/{name}/library", delete(delete_datapack))
        .route("/addons", get(list_addons))
        .route("/addons/upload", post(upload_addon))
        .route("/addons/search", get(search_addons))
        .route("/addons/install", post(install_addon))
        .route("/addons/updates", get(addon_updates))
        .route("/addons/{name}", delete(delete_addon))
        .route("/addons/{name}/enabled", post(toggle_addon))
        .route("/addons/{name}/update", post(update_addon))
        .route("/history", get(history))
        .route("/gamerules", get(get_gamerules).post(set_gamerule))
        .route("/properties", get(get_properties).post(set_properties))
        .route("/icon", get(get_icon).put(put_icon).delete(reset_icon))
        .route("/port", put(set_port));

    let protected = Router::new()
        .route("/api/versions", get(list_versions))
        .route("/api/versions/{flavor}/{version}", delete(delete_version))
        .route("/api/servers", get(list_servers).post(create_server))
        .route(
            "/api/servers/{sid}",
            axum::routing::patch(rename_server).delete(delete_server),
        )
        .route(
            "/api/map/visibility",
            get(get_map_visibility).put(put_map_visibility),
        )
        .nest("/api/servers/{sid}", srv)
        .layer(middleware::from_fn(require_auth));

    Router::new()
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/health", get(health))
        .route("/ws", get(websocket))
        .route("/", get(index))
        .route("/static/{*path}", get(static_file))
        .merge(protected)
        .fallback(|| async { (StatusCode::NOT_FOUND, Json(json!({"detail": "Not Found"}))) })
        .layer(axum::extract::DefaultBodyLimit::max(1 << 20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; panel_session=tok-en_1; b=2"),
        );
        assert_eq!(cookie(&h).as_deref(), Some("tok-en_1"));
        assert_eq!(cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn errors_map_to_status_codes() {
        let s = |e: Error| ApiError::from(e).0;
        assert_eq!(s(Error::Busy("x".into())), StatusCode::CONFLICT);
        assert_eq!(s(Error::Invalid("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(s(Error::NotFound("x".into())), StatusCode::NOT_FOUND);
        assert_eq!(s(Error::InUse("x".into())), StatusCode::CONFLICT);
        assert_eq!(s(Error::Remote("x".into())), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn routes_build() {
        let _ = router();
    }
}
