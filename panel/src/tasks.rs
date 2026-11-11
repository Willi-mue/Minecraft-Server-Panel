//! Background loops: live stats, history, TPS, map rendering, map player positions, scheduled
//! backups. Each does nothing when nobody is watching (idle cost is a stated feature).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::api::HISTORY_INTERVAL;
use crate::config::cfg;
use crate::livemap;
use crate::server::{self, HUB, State};
use crate::util::{now_secs, round_to};

/// Inside a container the network counters only cover this container, i.e. exactly Minecraft +
/// panel. On a normal host they would be machine-wide, so they're omitted there.
fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
}

fn cpu_count() -> f64 {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1) as f64
}

/// CPU as % of the whole machine (not per core) and resident memory in MB of the given processes.
/// CPU comes from the difference of the process's CPU time between two samples, so the first
/// sample of a process only primes it (0).
struct Meter {
    sys: System,
    last: HashMap<u32, (u64, Instant)>, // pid -> (CPU time in ms, when)
}

impl Meter {
    fn new() -> Meter {
        Meter {
            sys: System::new(),
            last: HashMap::new(),
        }
    }

    fn sample(&mut self, pids: &[u32]) -> HashMap<u32, Value> {
        let list: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&list),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let cpus = cpu_count();
        let now = Instant::now();
        let mut out = HashMap::new();
        for p in pids {
            let Some(proc_) = self.sys.process(Pid::from_u32(*p)) else {
                self.last.remove(p);
                continue;
            };
            let used = proc_.accumulated_cpu_time();
            let cpu = match self.last.insert(*p, (used, now)) {
                Some((before, t)) => {
                    let wall_ms = now.duration_since(t).as_secs_f64() * 1000.0;
                    if wall_ms > 0.0 {
                        used.saturating_sub(before) as f64 / wall_ms / cpus * 100.0
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            out.insert(
                *p,
                json!({"cpu": round_to(cpu.min(100.0), 1), "ram": proc_.memory() >> 20}),
            );
        }
        self.last.retain(|pid, _| pids.contains(pid));
        out
    }
}

fn net_totals(n: &mut Networks) -> (u64, u64) {
    n.refresh(true);
    n.iter().fold((0, 0), |(up, down), (_, d)| {
        (up + d.total_transmitted(), down + d.total_received())
    })
}

pub async fn stats_loop() {
    let me = std::process::id();
    let mut meter = Meter::new();
    let mut known: HashMap<String, u32> = HashMap::new(); // server -> pid already primed
    let mut net = in_container().then(Networks::new_with_refreshed_list);
    let mut last_net = net.as_mut().map(net_totals);
    let mut last = Instant::now();
    meter.sample(&[me]);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if !HUB.has_subscribers() {
            // Nobody is watching: skip the work (keep the baseline fresh for the next sample)
            last = Instant::now();
            last_net = net.as_mut().map(net_totals);
            continue;
        }
        let elapsed = last.elapsed().as_secs_f64().max(0.001);
        last = Instant::now();
        let managers = server::all();
        let usages: Vec<_> = managers.iter().map(|m| (m.clone(), m.usage())).collect();
        let mut pids: Vec<u32> = vec![me];
        pids.extend(usages.iter().filter_map(|(_, u)| u.pid));
        let samples = meter.sample(&pids);
        let mut net_json = Value::Null;
        if let (Some(n), Some((up0, down0))) = (net.as_mut(), last_net) {
            let (up, down) = net_totals(n);
            net_json = json!({
                "up": round_to(up.saturating_sub(up0) as f64 / elapsed / 1024.0, 1),
                "down": round_to(down.saturating_sub(down0) as f64 / elapsed / 1024.0, 1),
            });
            last_net = Some((up, down));
        }
        let panel = samples
            .get(&me)
            .cloned()
            .unwrap_or(json!({"cpu": 0.0, "ram": 0}));
        for (m, u) in usages {
            let mut mc = Value::Null;
            match u.pid {
                None => {
                    known.remove(&m.inst.id);
                }
                Some(pid) => {
                    if known.get(&m.inst.id) != Some(&pid) {
                        known.insert(m.inst.id.clone(), pid); // first sample only primes the CPU measurement
                    }
                    if let Some(s) = samples.get(&pid) {
                        mc = s.clone();
                        mc["ram_max"] = json!(u.max_ram_mb);
                    }
                }
            }
            let tick = u.tick.filter(|t| now_secs() - t.t < 30.0).map(|t| t.json());
            HUB.emit(&json!({
                "type": "stats", "server": m.inst.id,
                "stats": {"minecraft": mc, "uptime": u.uptime, "tick": tick, "panel": panel, "net": net_json},
            }));
        }
    }
}

/// One point per running server every 30 s for the last hour (CPU averaged over the interval).
/// Stopped servers cost nothing; TPS is only there while somebody had the panel open.
pub async fn history_loop() {
    let mut meter = Meter::new();
    let mut primed: HashMap<String, u32> = HashMap::new();
    loop {
        tokio::time::sleep(Duration::from_secs(HISTORY_INTERVAL)).await;
        let usages: Vec<_> = server::all()
            .into_iter()
            .map(|m| {
                let u = m.usage();
                (m, u)
            })
            .collect();
        let pids: Vec<u32> = usages.iter().filter_map(|(_, u)| u.pid).collect();
        let samples = meter.sample(&pids);
        for (m, u) in usages {
            let Some(pid) = u.pid else {
                primed.remove(&m.inst.id);
                m.clear_history();
                continue;
            };
            if primed.get(&m.inst.id) != Some(&pid) {
                primed.insert(m.inst.id.clone(), pid);
                continue; // needs one interval as baseline
            }
            let Some(s) = samples.get(&pid) else { continue };
            let tick = u
                .tick
                .filter(|t| now_secs() - t.t < HISTORY_INTERVAL as f64 + 5.0);
            m.push_history(json!({
                "t": now_secs() as i64, "cpu": s["cpu"], "ram": s["ram"], "players": u.players,
                "tps": tick.and_then(|t| t.tps), "mspt": tick.and_then(|t| t.mspt),
            }));
        }
    }
}

/// TPS/MSPT every 10 s, only while somebody has the panel open (no console traffic otherwise).
pub async fn tick_loop() {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        if !HUB.has_subscribers() {
            continue;
        }
        for m in server::all() {
            if m.state() == State::Running && m.task().is_none() {
                let _ = tokio::task::spawn_blocking(move || m.query_tick()).await;
            }
        }
    }
}

/// Keep the map current: once at startup, then every MAP_INTERVAL s while the server runs. mapgen
/// only touches changed region files, so a check without changes costs milliseconds. Every server
/// with the map turned on, one after the other.
pub async fn map_render_loop() {
    let mut first = true;
    loop {
        for m in server::all() {
            if m.map_enabled.load(Ordering::SeqCst) && (first || m.state() == State::Running) {
                let _ = tokio::task::spawn_blocking(move || {
                    let had_map = livemap::available(&m.inst);
                    livemap::renderer_for(&m.inst).render(&|t| m.info(t), false, false);
                    if !had_map && livemap::available(&m.inst) {
                        m.emit_status(); // map link appears
                    }
                })
                .await;
            }
        }
        first = false;
        tokio::time::sleep(Duration::from_secs(cfg().map_interval)).await;
    }
}

/// Live player positions, only while somebody has the map open (no idle console traffic).
pub async fn map_players_loop() {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        for m in server::all() {
            let tracker = livemap::tracker_for(&m.inst);
            if !(m.map_enabled.load(Ordering::SeqCst)
                && m.state() == State::Running
                && tracker.watching())
            {
                continue;
            }
            for cmd in tracker.queries(&m.players()) {
                if m.send_command(&cmd).is_err() {
                    break;
                }
            }
        }
    }
}

/// Automatic backups every BACKUP_INTERVAL_HOURS, only for running servers (a stopped world doesn't
/// change, and extra copies would rotate out older good ones).
pub async fn backup_schedule() {
    let hours = cfg().backup_interval_hours;
    loop {
        tokio::time::sleep(Duration::from_secs_f64(hours * 3600.0)).await;
        for m in server::all() {
            if m.state() != State::Running {
                continue;
            }
            m.info("Scheduled backup");
            if m.make_backup().is_err() {
                m.info("Scheduled backup skipped: another action is running.");
            }
        }
    }
}

/// Pick up Mojang jars Paper downloaded in earlier runs (needs network).
pub fn import_paper_caches() {
    for m in server::all() {
        crate::versions::import_paper_cache(&m.inst.server_dir(), &|t| m.info(t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_of_a_busy_process_is_measured() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let t = std::thread::spawn(move || {
            let mut x = 0u64;
            while !s.load(Ordering::Relaxed) {
                x = x.wrapping_mul(31).wrapping_add(7);
            }
            x
        });
        let mut m = Meter::new();
        let me = std::process::id();
        m.sample(&[me]);
        std::thread::sleep(Duration::from_millis(1500));
        let s = m.sample(&[me]);
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let cpu = s[&me]["cpu"].as_f64().unwrap();
        assert!(cpu > 100.0 / cpu_count() * 0.5, "one busy core, got {cpu}");
        assert!(s[&me]["ram"].as_u64().unwrap() > 0);
    }
}
