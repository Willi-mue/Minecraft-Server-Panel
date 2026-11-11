//! Outgoing HTTPS requests (Mojang, PaperMC, Fabric, Modrinth). Blocking: called from worker
//! threads, never on the async runtime.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::Value;
use sha1::Digest;
use ureq::Agent;

use crate::error::{Error, Result};
use crate::util::with_suffix;

// PaperMC asks API clients to identify themselves with a descriptive User-Agent
pub const USER_AGENT: &str = "minecraft-server-panel/1.0 (self-hosted server panel)";

fn agent(timeout: u64) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout)))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

static SHORT: LazyLock<Agent> = LazyLock::new(|| agent(20));
static LONG: LazyLock<Agent> = LazyLock::new(|| agent(600));

fn net(url: &str, e: ureq::Error) -> Error {
    Error::Remote(format!("{}: {e}", host(url)))
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

fn check(url: &str, status: u16) -> Result<()> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(Error::Status(status, host(url)))
    }
}

/// GET a JSON document (HTTP errors as `Error::Status`).
pub fn get_json(url: &str) -> Result<Value> {
    let mut resp = SHORT.get(url).call().map_err(|e| net(url, e))?;
    check(url, resp.status().as_u16())?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| net(url, e))?;
    serde_json::from_str(&text)
        .map_err(|e| Error::Remote(format!("Invalid JSON from {}: {e}", host(url))))
}

/// GET returning (status, JSON or Null) without treating 204/404 as errors.
pub fn get_json_status(url: &str) -> Result<(u16, Value)> {
    let mut resp = SHORT.get(url).call().map_err(|e| net(url, e))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

/// POST JSON, answer as JSON.
pub fn post_json(url: &str, body: &Value) -> Result<Value> {
    let mut resp = SHORT.post(url).send_json(body).map_err(|e| net(url, e))?;
    check(url, resp.status().as_u16())?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| net(url, e))?;
    serde_json::from_str(&text)
        .map_err(|e| Error::Remote(format!("Invalid JSON from {}: {e}", host(url))))
}

/// GET a small binary (at most `limit` bytes, else an error). Status must be 200.
pub fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut resp = SHORT.get(url).call().map_err(|e| net(url, e))?;
    if resp.status().as_u16() != 200 {
        return Err(Error::Status(resp.status().as_u16(), host(url)));
    }
    let mut data = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| Error::Remote(e.to_string()))?;
    if data.len() as u64 > limit {
        return Err(Error::Remote("response too large".into()));
    }
    Ok(data)
}

pub enum Hash {
    Sha1(String),
    Sha256(String),
    Sha512(String),
    /// No checksum published: only a size limit
    None,
}

/// Stream a download to `<target>.part` (or `part` if given), check size + hash, then move it
/// into place. The partial file is always removed.
pub fn download(
    url: &str,
    target: &Path,
    part: Option<&Path>,
    expected: Hash,
    max: u64,
) -> Result<()> {
    let tmp = part
        .map(Path::to_path_buf)
        .unwrap_or_else(|| with_suffix(target, ".part"));
    let result = (|| -> Result<()> {
        let mut resp = LONG.get(url).call().map_err(|e| net(url, e))?;
        check(url, resp.status().as_u16())?;
        let mut reader = resp.body_mut().as_reader();
        let mut file = std::fs::File::create(&tmp)?;
        let mut s1 = sha1::Sha1::new();
        let mut s256 = sha2::Sha256::new();
        let mut s512 = sha2::Sha512::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut size = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| Error::Remote(format!("Download failed: {e}")))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > max {
                return Err(Error::Invalid("Download too large, discarded".into()));
            }
            match expected {
                Hash::Sha1(_) => s1.update(&buf[..n]),
                Hash::Sha256(_) => s256.update(&buf[..n]),
                Hash::Sha512(_) => s512.update(&buf[..n]),
                Hash::None => {}
            }
            file.write_all(&buf[..n])?;
        }
        file.flush()?;
        drop(file);
        let ok = match &expected {
            Hash::Sha1(h) => hex::encode(s1.finalize()) == h.to_lowercase(),
            Hash::Sha256(h) => hex::encode(s256.finalize()) == h.to_lowercase(),
            Hash::Sha512(h) => hex::encode(s512.finalize()) == h.to_lowercase(),
            Hash::None => true,
        };
        if !ok {
            return Err(Error::Invalid(
                "Checksum mismatch, download discarded".into(),
            ));
        }
        std::fs::rename(&tmp, target)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Hex digest of a file.
pub fn file_hash<D: Digest>(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut d = D::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        d.update(&buf[..n]);
    }
    Ok(hex::encode(d.finalize()))
}

/// Percent-encode one URL path segment / query value.
pub fn quote(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// `a=1&b=2` from pairs.
pub fn query(pairs: &[(&str, String)]) -> String {
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(k, v);
    }
    ser.finish()
}
