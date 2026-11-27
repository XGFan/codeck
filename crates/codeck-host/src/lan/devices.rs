//! Paired LAN devices persisted in `devices.json`, with per-device revocation signals.

use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

const DEVICES_FILE: &str = "devices.json";
const MAX_NAME_CHARS: usize = 64;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A paired client device, exposed in IPC and `codeck-host devices`.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    /// Epoch milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub paired_at: f64,
    /// Epoch milliseconds of the last WebSocket connection, if any.
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_seen_at: Option<f64>,
}

/// On-disk record including the hashed bearer token; kept separate from [`PairedDevice`].
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct StoredDevice {
    id: String,
    name: String,
    paired_at: i64,
    last_seen_at: Option<i64>,
    token_sha256: String,
}

impl StoredDevice {
    fn to_public(&self) -> PairedDevice {
        PairedDevice {
            id: self.id.clone(),
            name: self.name.clone(),
            paired_at: self.paired_at as f64,
            last_seen_at: self.last_seen_at.map(|ms| ms as f64),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DevicesFile {
    #[serde(default)]
    devices: Vec<StoredDevice>,
}

/// Per-device revocation signal: flipped once when the device is removed so all its active HTTP
/// requests and WebSocket connections terminate immediately.
#[derive(Clone, Default)]
pub struct DeviceRevoke(watch::Sender<bool>);

impl DeviceRevoke {
    pub fn revoke(&self) {
        self.0.send_replace(true);
    }

    pub fn is_revoked(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once revoked, or immediately if already revoked.
    pub async fn revoked(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|revoked| *revoked).await;
    }
}

/// Normalizes a client-supplied device name: trims whitespace, falls back to `"iPhone"` if empty,
/// and truncates at 64 Unicode scalar values.
pub fn normalize_device_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "iPhone".to_string();
    }
    trimmed.chars().take(MAX_NAME_CHARS).collect()
}

pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut buf = [0u8; N];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| "系统随机数生成失败".to_string())?;
    Ok(buf)
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn sha256_hex(input: &[u8]) -> String {
    hex_lower(&Sha256::digest(input))
}

/// In-memory device table backed by `<dir>/devices.json`.
pub struct Devices {
    path: PathBuf,
    devices: Vec<StoredDevice>,
    revokes: HashMap<String, DeviceRevoke>,
}

impl Devices {
    /// Loads `<dir>/devices.json`; a missing file is treated as an empty list without creating anything.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(DEVICES_FILE);
        let devices = match std::fs::read(&path) {
            Ok(bytes) => {
                let file: DevicesFile = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("解析设备列表失败（{}）：{e}", path.display()))?;
                file.devices
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("读取设备列表失败（{}）：{e}", path.display())),
        };
        let revokes = devices
            .iter()
            .map(|d| (d.id.clone(), DeviceRevoke::default()))
            .collect();
        Ok(Self {
            path,
            devices,
            revokes,
        })
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        self.devices.iter().map(StoredDevice::to_public).collect()
    }

    /// Generates a new device id + bearer token, persists `devices.json` atomically, and only then
    /// commits the entry in memory.
    pub fn add(&mut self, name: &str, now_ms: i64) -> Result<(PairedDevice, String), String> {
        let id = loop {
            let candidate = hex_lower(&random_bytes::<6>()?);
            if !self.devices.iter().any(|d| d.id == candidate) {
                break candidate;
            }
        };
        let token_bytes = random_bytes::<32>()?;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes);
        let record = StoredDevice {
            id: id.clone(),
            name: normalize_device_name(name),
            paired_at: now_ms,
            last_seen_at: None,
            token_sha256: sha256_hex(token.as_bytes()),
        };
        let mut next = self.devices.clone();
        let public = record.to_public();
        next.push(record);
        save_atomic(&self.path, &next)?;
        self.devices = next;
        self.revokes.insert(id, DeviceRevoke::default());
        Ok((public, token))
    }

    /// Verifies a bearer token, returning the device id and its revocation handle.
    pub fn verify(&self, token: &str) -> Option<(String, DeviceRevoke)> {
        if token.is_empty() {
            return None;
        }
        let hash = sha256_hex(token.as_bytes());
        let dev = self.devices.iter().find(|d| d.token_sha256 == hash)?;
        let revoke = self.revokes.get(&dev.id)?.clone();
        Some((dev.id.clone(), revoke))
    }

    /// Removes a device, persisting first and then firing its revocation signal. Returns `Ok(None)`
    /// when `id` is not present.
    pub fn remove_device(&mut self, id: &str) -> Result<Option<PairedDevice>, String> {
        let Some(pos) = self.devices.iter().position(|d| d.id == id) else {
            return Ok(None);
        };
        let mut next = self.devices.clone();
        let removed = next.remove(pos).to_public();
        save_atomic(&self.path, &next)?;
        self.devices = next;
        if let Some(revoke) = self.revokes.remove(id) {
            revoke.revoke();
        }
        Ok(Some(removed))
    }

    /// Removes a device, persisting first and then firing its revocation signal. Returns `Ok(false)`
    /// when `id` is not present.
    pub fn remove(&mut self, id: &str) -> Result<bool, String> {
        Ok(self.remove_device(id)?.is_some())
    }

    /// Updates `lastSeenAt` for `id`, committing only after persistence succeeds; write failures
    /// are logged and ignored so connections stay up.
    pub fn touch(&mut self, id: &str, now_ms: i64) -> bool {
        let Some(pos) = self.devices.iter().position(|d| d.id == id) else {
            return false;
        };
        let mut next = self.devices.clone();
        next[pos].last_seen_at = Some(now_ms);
        if let Err(e) = save_atomic(&self.path, &next) {
            tracing::warn!("failed to persist devices.json after touch: {e}");
            return false;
        }
        self.devices = next;
        true
    }
}

fn save_atomic(path: &Path, devices: &[StoredDevice]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("创建设备列表目录失败（{}）：{e}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(&DevicesFile {
        devices: devices.to_vec(),
    })
    .map_err(|e| format!("序列化设备列表失败：{e}"))?;

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!("{DEVICES_FILE}.tmp-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let write_res = (|| -> io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入临时设备列表失败（{}）：{e}", tmp.display()));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换设备列表失败（{}）：{e}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_device_name() {
        assert_eq!(normalize_device_name("  "), "iPhone");
        assert_eq!(normalize_device_name("  My iPhone \n"), "My iPhone");
        let long = "测".repeat(80);
        let got = normalize_device_name(&long);
        assert_eq!(got.chars().count(), 64);
    }

    #[test]
    fn add_verify_touch_reload_and_remove_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let mut devices = Devices::load(dir.path()).unwrap();
        assert!(devices.list().is_empty());
        assert!(!dir.path().join("devices.json").exists());

        let (dev, token) = devices.add("  iPhone 16  ", 1_700_000_000_000).unwrap();
        assert_eq!(dev.id.len(), 12);
        assert!(
            dev.id
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );
        assert_eq!(dev.name, "iPhone 16");
        assert_eq!(dev.paired_at, 1_700_000_000_000.0);
        assert_eq!(dev.last_seen_at, None);
        assert_eq!(token.len(), 43);

        let (id, revoke) = devices.verify(&token).unwrap();
        assert_eq!(id, dev.id);
        assert!(!revoke.is_revoked());
        assert!(devices.verify("wrong").is_none());

        assert!(devices.touch(&dev.id, 1_700_000_005_000));
        let reloaded = Devices::load(dir.path()).unwrap();
        assert_eq!(reloaded.list()[0].last_seen_at, Some(1_700_000_005_000.0));
        assert_eq!(reloaded.verify(&token).unwrap().0, dev.id);

        assert!(devices.remove(&dev.id).unwrap());
        assert!(revoke.is_revoked());
        assert!(devices.verify(&token).is_none());
        assert!(!devices.remove(&dev.id).unwrap());

        let reloaded = Devices::load(dir.path()).unwrap();
        assert!(reloaded.list().is_empty());
        assert!(reloaded.verify(&token).is_none());
    }
}
