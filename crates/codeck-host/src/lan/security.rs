//! Combined TLS certificate, paired device table, and active pairing offer for non-loopback LAN listeners.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rustls::ServerConfig;

use super::control::{DaemonLock, lock_state_dir, normalize_state_dir};
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

/// Holds the Host's lazily loaded TLS certificate alongside the paired device table and active
/// pairing offer, guarded by an exclusive `flock` on `<dir>/daemon.lock`.
pub struct LanSecurity {
    dir: PathBuf,
    _lock: DaemonLock,
    /// Lazily loaded on first TLS start; errors are not cached so fixing `tls/` and retrying works.
    cert: Mutex<Option<Arc<HostCert>>>,
    /// Serializes all device table and pairing state mutations (including `devices.json` writes).
    state: Mutex<SecurityState>,
}

impl LanSecurity {
    /// Normalizes `dir`, sets it to `0700`, acquires `<dir>/daemon.lock`, and opens the device
    /// table without loading or generating the TLS certificate.
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        let dir = normalize_state_dir(&dir)?;
        let lock = lock_state_dir(&dir)?;
        let devices = Devices::load(&dir)?;
        Ok(Self {
            dir,
            _lock: lock,
            cert: Mutex::new(None),
            state: Mutex::new(SecurityState {
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
        self.state.lock().devices.list()
    }

    pub fn start_pairing(&self) -> Result<StartedOffer, String> {
        self.start_pairing_ttl(PAIRING_TTL)
    }

    pub fn start_pairing_ttl(&self, ttl: Duration) -> Result<StartedOffer, String> {
        let mut st = self.state.lock();
        st.pairing.start(Instant::now(), now_ms(), ttl)
    }

    pub fn start_pairing_with_ttl(
        &self,
        now: Instant,
        now_ms: i64,
        ttl: Duration,
    ) -> Result<StartedOffer, String> {
        self.state.lock().pairing.start(now, now_ms, ttl)
    }

    pub fn cancel_pairing(&self, id: &str) -> bool {
        self.state.lock().pairing.cancel(id)
    }

    pub fn cancel_any_pairing(&self) {
        self.state.lock().pairing.cancel_any();
    }

    #[cfg(test)]
    pub(crate) fn has_active_pairing(&self) -> bool {
        self.state.lock().pairing.is_active()
    }

    pub fn redeem(&self, key: &str, name: &str) -> Result<RedeemSuccess, RedeemError> {
        let mut st = self.state.lock();
        // Sample the clock only after acquiring the lock so contention cannot extend TTL.
        Self::redeem_locked(&mut st, key, name, Instant::now(), now_ms())
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
        let mut st = self.state.lock();
        Self::redeem_locked(&mut st, key, name, now, now_ms)
    }

    fn redeem_locked(
        st: &mut SecurityState,
        key: &str,
        name: &str,
        now: Instant,
        now_ms: i64,
    ) -> Result<RedeemSuccess, RedeemError> {
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
            .expect("offer verified under state lock");
        let devices = st.devices.list();
        Ok(RedeemSuccess {
            offer_id,
            device,
            token,
            devices,
        })
    }

    /// Verifies a bearer token and returns `(device_id, revoke_signal)`.
    pub fn verify(&self, token: &str) -> Option<(String, DeviceRevoke)> {
        self.state.lock().devices.verify(token)
    }

    /// Removes a device, persisting before committing and firing its revocation signal. Returns
    /// `(removed_device, updated_list)`.
    pub fn remove_device(
        &self,
        id: &str,
    ) -> Result<(Option<PairedDevice>, Vec<PairedDevice>), String> {
        let mut st = self.state.lock();
        let removed = st.devices.remove_device(id)?;
        let devices = st.devices.list();
        Ok((removed, devices))
    }

    /// Removes a device, firing its revocation signal, and returns `(was_present, updated_list)`.
    pub fn remove(&self, id: &str) -> Result<(bool, Vec<PairedDevice>), String> {
        let (removed, devices) = self.remove_device(id)?;
        Ok((removed.is_some(), devices))
    }

    /// Updates `lastSeenAt` for `id`, returning the updated device list when persisted.
    pub fn touch(&self, id: &str) -> Option<Vec<PairedDevice>> {
        self.touch_at(id, now_ms())
    }

    pub fn touch_at(&self, id: &str, now_ms: i64) -> Option<Vec<PairedDevice>> {
        let mut st = self.state.lock();
        st.devices.touch(id, now_ms).then(|| st.devices.list())
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
