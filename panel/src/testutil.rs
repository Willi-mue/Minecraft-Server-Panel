//! Test helpers: a fresh data directory per test. Tests that touch the configuration run one at a
//! time (the configuration is global).

use std::sync::{Mutex, MutexGuard};

use crate::config::{self, Config};

static SERIAL: Mutex<()> = Mutex::new(());

pub struct DataDir {
    #[allow(dead_code)]
    pub dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

/// Point all data paths at a fresh temporary directory (held until the guard is dropped).
pub fn data_dir() -> DataDir {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    config::set(Config::for_dir(dir.path()));
    config::ensure_dirs().unwrap();
    crate::sessions::reset_cache();
    DataDir { dir, _guard: guard }
}
