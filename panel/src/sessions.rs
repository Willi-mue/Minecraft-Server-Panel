//! Login sessions that survive a panel restart.
//!
//! Only a SHA-256 of each token is stored (`DATA_DIR/sessions.json`), so the file alone can't be
//! used to log in. The file also holds a fingerprint of the panel password: changing PANEL_PASSWORD
//! logs everybody out. Sessions slide: using one renews it once less than half of its lifetime is left.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::cfg;
use crate::util::{ct_eq, lock, now_secs, read_json, token};

pub const TTL: u64 = 30 * 24 * 3600;

/// token hash -> expiry (loaded lazily)
static SESSIONS: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);

#[derive(Debug, PartialEq)]
pub enum Check {
    Invalid,
    Ok,
    /// Valid and its lifetime was just extended (the caller re-sends the cookie)
    Renewed,
}

fn file() -> PathBuf {
    cfg().data_dir.join("sessions.json")
}

fn fingerprint() -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(cfg().password.as_bytes()).expect("any key length");
    mac.update(b"panel-sessions");
    hex::encode(mac.finalize().into_bytes())
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn load(slot: &mut Option<HashMap<String, f64>>) -> &mut HashMap<String, f64> {
    slot.get_or_insert_with(|| {
        let mut out = HashMap::new();
        if let Some(data) = read_json(&file()) {
            let stored = data.get("password").and_then(Value::as_str).unwrap_or("");
            if ct_eq(stored.as_bytes(), fingerprint().as_bytes()) {
                let now = now_secs();
                if let Some(Value::Object(s)) = data.get("sessions") {
                    for (h, exp) in s {
                        if let Some(exp) = exp.as_f64().filter(|e| *e > now) {
                            out.insert(h.clone(), exp);
                        }
                    }
                }
            }
        }
        out
    })
}

fn save(sessions: &HashMap<String, f64>) {
    let path = file();
    let data = json!({"password": fingerprint(), "sessions": sessions});
    let tmp = path.with_extension("tmp");
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp, data.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)
    })();
    let _ = result; // still works for this run, only the restart survival is lost
}

pub fn create() -> String {
    let t = token(32);
    let now = now_secs();
    let mut slot = lock(&SESSIONS);
    let s = load(&mut slot);
    s.retain(|_, exp| *exp >= now);
    s.insert(hash(&t), now + TTL as f64);
    save(s);
    t
}

pub fn check(token: Option<&str>) -> Check {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return Check::Invalid;
    };
    let h = hash(token);
    let now = now_secs();
    let mut slot = lock(&SESSIONS);
    let s = load(&mut slot);
    match s.get(&h).copied() {
        Some(exp) if exp >= now => {
            if exp - now < TTL as f64 / 2.0 {
                s.insert(h, now + TTL as f64);
                save(s);
                Check::Renewed
            } else {
                Check::Ok
            }
        }
        Some(_) => {
            s.remove(&h);
            save(s);
            Check::Invalid
        }
        None => Check::Invalid,
    }
}

pub fn remove(token: Option<&str>) {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return;
    };
    let mut slot = lock(&SESSIONS);
    let s = load(&mut slot);
    if s.remove(&hash(token)).is_some() {
        save(s);
    }
}

/// Tests: forget the loaded sessions (re-read from the file).
#[cfg(test)]
pub fn reset_cache() {
    *lock(&SESSIONS) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survive_restart_and_password_change() {
        let _t = crate::testutil::data_dir();
        reset_cache();
        let t = create();
        assert_eq!(check(Some(&t)), Check::Ok);
        assert_eq!(check(Some("wrong")), Check::Invalid);
        assert_eq!(check(None), Check::Invalid);
        reset_cache();
        assert_eq!(check(Some(&t)), Check::Ok, "survives a restart");
        let text = std::fs::read_to_string(file()).unwrap();
        assert!(!text.contains(&t), "only hashes are stored");

        let mut c = (*cfg()).clone();
        c.password = "another".into();
        crate::config::set(c);
        reset_cache();
        assert_eq!(
            check(Some(&t)),
            Check::Invalid,
            "new password logs everybody out"
        );
        reset_cache();
    }

    #[test]
    fn sliding_renewal_and_logout() {
        let _t = crate::testutil::data_dir();
        reset_cache();
        let t = create();
        {
            let mut slot = lock(&SESSIONS);
            let s = load(&mut slot);
            for exp in s.values_mut() {
                *exp = now_secs() + 100.0;
            }
        }
        assert_eq!(check(Some(&t)), Check::Renewed);
        assert_eq!(check(Some(&t)), Check::Ok);
        remove(Some(&t));
        assert_eq!(check(Some(&t)), Check::Invalid);
        reset_cache();
    }
}
