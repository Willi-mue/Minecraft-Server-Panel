//! Minecraft Server Panel: web panel that runs and controls Minecraft Java servers (Vanilla, Paper,
//! Fabric) as child processes. Admin panel on WEB_PORT, public live map on MAP_PORT.

mod access;
mod addons;
mod api;
mod backup;
mod config;
mod datapacks;
mod error;
mod gamerules;
mod http;
mod instances;
mod java;
mod leaderboard;
mod livemap;
mod mapserver;
mod nbt;
mod players;
mod properties;
mod server;
mod servericon;
mod sessions;
mod skins;
mod statics;
mod tasks;
mod util;
mod versions;
mod waypoints;
mod world;

#[cfg(test)]
mod testutil;

use std::net::SocketAddr;
use std::time::Duration;

/// `panel health`: exit 0 if the panel answers on WEB_PORT (Docker's HEALTHCHECK, no curl needed).
fn health_check() -> ! {
    use std::io::{Read, Write};
    let port = std::env::var("WEB_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(47318);
    let ok = (|| -> std::io::Result<bool> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(4))?;
        s.set_read_timeout(Some(Duration::from_secs(4)))?;
        s.write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut answer = String::new();
        s.read_to_string(&mut answer)?;
        Ok(answer.starts_with("HTTP/1.1 200"))
    })()
    .unwrap_or(false);
    std::process::exit(if ok { 0 } else { 1 });
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("health") {
        health_check();
    }
    let c = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };
    if c.password.is_empty() {
        eprintln!(
            "ERROR: PANEL_PASSWORD is not set. Set a panel password (e.g. in .env) and start again."
        );
        std::process::exit(1);
    }
    config::set(c);
    let c = config::cfg();
    if let Err(e) = config::ensure_dirs() {
        eprintln!(
            "ERROR: could not create the data folders in {}: {e}",
            c.data_dir.display()
        );
        std::process::exit(1);
    }
    if !c.static_dir.join("index.html").is_file() {
        warn!(
            "frontend not found in {} (set STATIC_DIR)",
            c.static_dir.display()
        );
    }
    server::load_managers();
    leaderboard::set_save_hook(server::leaderboard_save);

    tokio::task::spawn_blocking(tasks::import_paper_caches);
    let mut loops = vec![
        tokio::spawn(tasks::stats_loop()),
        tokio::spawn(tasks::map_render_loop()),
        tokio::spawn(tasks::map_players_loop()),
        tokio::spawn(tasks::history_loop()),
        tokio::spawn(tasks::tick_loop()),
    ];
    if c.backup_interval_hours > 0.0 {
        loops.push(tokio::spawn(tasks::backup_schedule()));
    }

    // The panel must work without the map
    let map_addr = format!("{}:{}", c.web_host, c.map_port);
    match tokio::net::TcpListener::bind(&map_addr).await {
        Ok(listener) => {
            let app = mapserver::router().into_make_service_with_connect_info::<SocketAddr>();
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, app).await {
                    warn!("live map server stopped: {e}");
                }
            });
        }
        Err(e) => warn!("live map server not started: {e}"),
    }

    let addr = format!("{}:{}", c.web_host, c.web_port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERROR: cannot listen on {addr}: {e}");
            std::process::exit(1);
        }
    };
    log!("panel on http://{addr}, live map on port {}", c.map_port);
    let app = api::router().into_make_service_with_connect_info::<SocketAddr>();
    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());
    if let Err(e) = serve.await {
        warn!("panel server stopped: {e}");
    }
    for l in loops {
        l.abort();
    }
    log!("stopping the Minecraft servers ...");
    let _ = tokio::time::timeout(
        Duration::from_secs(85),
        tokio::task::spawn_blocking(server::shutdown_all),
    )
    .await;
}

/// Ctrl+C or SIGTERM (`docker stop`). The server stops accepting, then the Minecraft servers are
/// stopped (open WebSockets don't block that: graceful shutdown only waits for in-flight requests
/// up to the point the process exits).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    log!("shutting down");
    api::SHUTDOWN.cancel();
}
