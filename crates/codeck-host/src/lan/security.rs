//! Combined TLS certificate, paired device table, and active pairing offer for non-loopback LAN listeners.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rustls::ServerConfig;

use super::devices::{DeviceRevoke, Devices, PairedDevice};
use super::pairing::{PAIRING_TTL, Pairing, StartedOffer};
use super::tls::HostCert;

#[derive(Debug)]
pub enum RedeemError {
    Expired,
    SaveFailed(String),
}

pub struct RedeemSuccess {
    pub offer_id: String,
    pub device: PairedDevice,
    pub token: String,
    pub devices: Vec<PairedDevice>,
}

struct SecurityState {
    devices: Devices,
    pairing: Pairing,
}

/// Holds the Host's lazily loaded TLS certificate alongside a single-lock state covering the paired
/// device table and active pairing offer.
pub struct LanSecurity {
    dir: PathBuf,
    /// Lazily loaded on first TLS start; errors are not cached so fixing `tls/` and retrying works.
    cert: Mutex<Option<Arc<HostCert>>>,
    /// Single lock serializing all device-table and pairing mutations and lookups.
    inner: Mutex<SecurityState>,
}

impl LanSecurity {
    /// Opens the device table in `dir` without loading or generating the TLS certificate.
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        let devices = Devices::load(&dir)?;
        Ok(Self {
            dir,
            cert: Mutex::new(None),
            inner: Mutex::new(SecurityState {
                devices,
                pairing: Pairing::default(),
            }),
        })
    }

    /// Loads or creates the TLS certificate once; errors are not cached.
    pub fn ensure_cert(&self) -> Result<Arc<HostCert>, String> {
        let mut slot = self.cert.lock();
        if let Some(cert) = slot.as_ref() {
            return Ok(Arc::clone(cert));
        }
        let cert = Arc::new(HostCert::load_or_create(&self.dir)?);
        *slot = Some(Arc::clone(&cert));
        Ok(cert)
    }

    pub fn server_config(&self) -> Result<Arc<ServerConfig>, String> {
        Ok(self.ensure_cert()?.server_config())
    }

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.inner.lock().devices.list()
    }

    pub fn start_pairing(&self) -> Result<StartedOffer, String> {
        self.start_pairing_with_ttl(Instant::now(), now_ms(), PAIRING_TTL)
    }

    pub fn start_pairing_with_ttl(
        &self,
        now: Instant,
        now_ms: i64,
        ttl: Duration,
    ) -> Result<StartedOffer, String> {
        self.inner.lock().pairing.start(now, now_ms, ttl)
    }

    pub fn cancel_pairing(&self, id: &str) -> bool {
        self.inner.lock().pairing.cancel(id)
    }

    pub fn cancel_any_pairing(&self) {
        self.inner.lock().pairing.cancel_any();
    }

    pub fn redeem(&self, key: &str, name: &str) -> Result<RedeemSuccess, RedeemError> {
        self.redeem_at(key, name, Instant::now(), now_ms())
    }

    /// Validates the offer, persists `devices.json` atomically, and only consumes the offer once the
    /// write succeeds. On save failure, the offer stays valid for retry.
    pub fn redeem_at(
        &self,
        key: &str,
        name: &str,
        now: Instant,
        now_ms: i64,
    ) -> Result<RedeemSuccess, RedeemError> {
        let mut st = self.inner.lock();
        if !st.pairing.check(key, now) {
            return Err(RedeemError::Expired);
        }
        let (device, token) = st
            .devices
            .add(name, now_ms)
            .map_err(RedeemError::SaveFailed)?;
        let offer_id = st
            .pairing
            .complete(device.clone())
            .expect("offer was checked under the same lock");
        let devices = st.devices.list();
        Ok(RedeemSuccess {
            offer_id,
            device,
            token,
            devices,
        })
    }

    /// Verifies a bearer token and returns `(device_id, revoke_signal)` under the same lock.
    pub fn verify(&self, token: &str) -> Option<(String, DeviceRevoke)> {
        self.inner.lock().devices.verify(token)
    }

    /// Removes a device, firing its revocation signal, and returns `(was_present, updated_list)`.
    pub fn remove(&self, id: &str) -> Result<(bool, Vec<PairedDevice>), String> {
        let mut st = self.inner.lock();
        let removed = st.devices.remove(id)?;
        let devices = st.devices.list();
        Ok((removed, devices))
    }

    pub fn touch(&self, id: &str) {
        self.touch_at(id, now_ms());
    }

    pub fn touch_at(&self, id: &str, now_ms: i64) {
        self.inner.lock().devices.touch(id, now_ms);
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    #[test]
    fn reading_devices_does_not_create_tls_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let sec = LanSecurity::open(dir.path().to_path_buf()).unwrap();
        assert!(sec.devices().is_empty());
        assert!(!dir.path().join("tls").exists());
    }

    #[test]
    fn concurrent_redeem_succeeds_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());
        let offer = sec.start_pairing().unwrap();
        let key = Arc::new(offer.key);
        let barrier = Arc::new(Barrier::new(8));

        let outcomes: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let sec = Arc::clone(&sec);
                    let key = Arc::clone(&key);
                    let barrier = Arc::clone(&barrier);
                    s.spawn(move || {
                        barrier.wait();
                        sec.redeem(&key, &format!("iPhone {i}"))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let ok_count = outcomes.iter().filter(|r| r.is_ok()).count();
        let exp_count = outcomes
            .iter()
            .filter(|r| matches!(r, Err(RedeemError::Expired)))
            .count();
        assert_eq!(ok_count, 1);
        assert_eq!(exp_count, 7);
        assert_eq!(sec.devices().len(), 1);
    }

    #[test]
    fn save_failure_keeps_offer_valid_for_retry() {
        let dir = tempfile::tempdir().unwrap();
        let sec = LanSecurity::open(dir.path().to_path_buf()).unwrap();
        let offer = sec.start_pairing().unwrap();

        // A directory at `devices.json` forces `fs::rename(tmp_file, devices.json)` to fail on Unix.
        let blocker = dir.path().join("devices.json");
        std::fs::create_dir(&blocker).unwrap();
        let err = sec.redeem(&offer.key, "iPhone").err().unwrap();
        assert!(matches!(err, RedeemError::SaveFailed(_)), "{err:?}");
        assert!(sec.devices().is_empty());

        std::fs::remove_dir(&blocker).unwrap();
        let ok = sec.redeem(&offer.key, "iPhone").unwrap();
        assert_eq!(ok.offer_id, offer.id);
        assert_eq!(sec.devices().len(), 1);
    }
}
