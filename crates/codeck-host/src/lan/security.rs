//! Combined TLS certificate, paired device table, and active pairing offer for non-loopback LAN listeners.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rustls::ServerConfig;

use super::control::{DaemonLock, lock_state_dir};
use super::devices::{DeviceRevoke, Devices, PairedDevice, StagedSnapshot, save_snapshot};
use super::pairing::{PAIRING_TTL, Pairing, StartedOffer};
use super::tls::HostCert;

#[derive(Debug)]
pub enum RedeemError {
    Expired,
    /// The HTTP request was abandoned before this redemption acquired the write lock.
    Aborted,
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
///
/// Disk writes (`redeem`, `remove_device`, `touch`) serialize on `io_lock` and run outside `state`,
/// so auth queries (`verify`, `devices`) and pairing cancellations never block on file I/O.
pub struct LanSecurity {
    dir: PathBuf,
    _lock: DaemonLock,
    /// Lazily loaded on first TLS start; errors are not cached so fixing `tls/` and retrying works.
    cert: Mutex<Option<Arc<HostCert>>>,
    /// Serializes disk-writing mutations (`redeem`, `remove_device`, `touch`); always acquired
    /// before `state` when both are needed.
    io_lock: Mutex<()>,
    /// Short-lived lock guarding in-memory devices and pairing state; never held across disk I/O.
    state: Mutex<SecurityState>,
    #[cfg(test)]
    before_save_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    before_publish_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl LanSecurity {
    /// Validates `dir` (non-empty, non-root, hop-by-hop trusted ancestors, 0700), acquires
    /// `<dir>/daemon.lock`, and opens the device table without loading or generating the TLS
    /// certificate. Subsequent file I/O uses the verified symlink-free canonical path.
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        let lock = lock_state_dir(&dir)?;
        let canonical_dir = lock.dir().to_path_buf();
        let devices = Devices::load(&canonical_dir)?;
        Ok(Self {
            dir: canonical_dir,
            _lock: lock,
            cert: Mutex::new(None),
            io_lock: Mutex::new(()),
            state: Mutex::new(SecurityState {
                devices,
                pairing: Pairing::default(),
            }),
            #[cfg(test)]
            before_save_hook: Mutex::new(None),
            #[cfg(test)]
            before_publish_hook: Mutex::new(None),
        })
    }

    /// Returns the verified symlink-free canonical state directory held by this instance.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[cfg(test)]
    pub(crate) fn set_before_save_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.before_save_hook.lock() = hook;
    }

    #[cfg(test)]
    fn run_before_save_hook(&self) {
        let hook = self.before_save_hook.lock().clone();
        if let Some(f) = hook {
            f();
        }
    }

    #[cfg(test)]
    pub(crate) fn set_before_publish_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.before_publish_hook.lock() = hook;
    }

    #[cfg(test)]
    fn run_before_publish_hook(&self) {
        let hook = self.before_publish_hook.lock().clone();
        if let Some(f) = hook {
            f();
        }
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
        self.redeem_impl(key, name, || (Instant::now(), now_ms()), None)
    }

    /// Like [`Self::redeem`], but aborts without committing if `cancelled` is already set when the
    /// write lock is acquired (e.g. the caller dropped the request while queued).
    pub fn redeem_cancellable(
        &self,
        key: &str,
        name: &str,
        cancelled: &AtomicBool,
    ) -> Result<RedeemSuccess, RedeemError> {
        self.redeem_impl(key, name, || (Instant::now(), now_ms()), Some(cancelled))
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
        self.redeem_impl(key, name, || (now, now_ms), None)
    }

    fn redeem_impl(
        &self,
        key: &str,
        name: &str,
        now_fn: impl FnOnce() -> (Instant, i64),
        cancelled: Option<&AtomicBool>,
    ) -> Result<RedeemSuccess, RedeemError> {
        let _io = self.io_lock.lock();
        if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(RedeemError::Aborted);
        }
        let (offer_id, pending, path) = {
            let mut st = self.state.lock();
            // Sample the clock only after acquiring the locks so contention cannot extend TTL.
            let (now, now_ms) = now_fn();
            let Some(offer_id) = st.pairing.check_offer_id(key, now) else {
                return Err(RedeemError::Expired);
            };
            let pending = st
                .devices
                .prepare_add(name, now_ms)
                .map_err(RedeemError::SaveFailed)?;
            let path = st.devices.path().to_path_buf();
            (offer_id, pending, path)
        };

        #[cfg(test)]
        self.run_before_save_hook();

        // Stage the snapshot to a temp file outside `self.state` so `verify` / `devices` never
        // block on file I/O; `devices.json` stays untouched until final checks pass under `state`.
        let staged =
            StagedSnapshot::stage(&path, &pending.next).map_err(RedeemError::SaveFailed)?;

        #[cfg(test)]
        self.run_before_publish_hook();

        let mut st = self.state.lock();
        if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
            drop(staged);
            return Err(RedeemError::Aborted);
        }
        if !st.pairing.is_active_id(&offer_id) {
            drop(staged);
            return Err(RedeemError::Expired);
        }
        staged.publish().map_err(RedeemError::SaveFailed)?;
        let id = st
            .pairing
            .complete_if_id(&offer_id, pending.public.clone())
            .expect("offer id verified under state lock");
        st.devices.commit_add(pending.next, pending.id);
        let devices = st.devices.list();
        Ok(RedeemSuccess {
            offer_id: id,
            device: pending.public,
            token: pending.token,
            devices,
        })
    }

    /// Verifies a bearer token and returns `(device_id, revoke_signal)` without waiting on disk I/O.
    pub fn verify(&self, token: &str) -> Option<(String, DeviceRevoke)> {
        self.state.lock().devices.verify(token)
    }

    /// Removes a device, persisting before committing and firing its revocation signal. Returns
    /// `(removed_device, updated_list)`.
    pub fn remove_device(
        &self,
        id: &str,
    ) -> Result<(Option<PairedDevice>, Vec<PairedDevice>), String> {
        let _io = self.io_lock.lock();
        let (removed, next, path) = {
            let st = self.state.lock();
            match st.devices.prepare_remove(id) {
                Some((removed, next)) => (removed, next, st.devices.path().to_path_buf()),
                None => return Ok((None, st.devices.list())),
            }
        };

        #[cfg(test)]
        self.run_before_save_hook();

        save_snapshot(&path, &next)?;

        let devices = {
            let mut st = self.state.lock();
            st.devices.commit_remove(id, next);
            st.devices.list()
        };
        Ok((Some(removed), devices))
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
        let _io = self.io_lock.lock();
        let (next, path) = {
            let st = self.state.lock();
            let next = st.devices.prepare_touch(id, now_ms)?;
            (next, st.devices.path().to_path_buf())
        };

        #[cfg(test)]
        self.run_before_save_hook();

        if let Err(e) = save_snapshot(&path, &next) {
            tracing::warn!("failed to persist devices.json after touch: {e}");
            return None;
        }

        let mut st = self.state.lock();
        st.devices.commit_touch(next);
        Some(st.devices.list())
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

    #[test]
    fn verify_and_devices_do_not_block_during_slow_disk_write() {
        let dir = tempfile::tempdir().unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());

        let o1 = sec.start_pairing().unwrap();
        let dev1 = sec.redeem(&o1.key, "Phone 1").unwrap();

        let o2 = sec.start_pairing().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        sec.set_before_save_hook(Some(Arc::new(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().recv();
        })));

        let sec_writer = Arc::clone(&sec);
        let key2 = o2.key.clone();
        let writer = std::thread::spawn(move || sec_writer.redeem(&key2, "Phone 2"));

        // Wait until the writer holds `io_lock` and is paused before disk I/O.
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Auth queries and device listing must succeed immediately without waiting on disk I/O,
        // and must not observe the uncommitted device yet.
        assert_eq!(sec.verify(&dev1.token).unwrap().0, dev1.device.id);
        assert_eq!(sec.devices().len(), 1);

        // A queued redemption whose cancel flag is already set aborts without committing once it
        // acquires `io_lock`.
        let cancelled = Arc::new(AtomicBool::new(true));
        let sec_queued = Arc::clone(&sec);
        let key2_again = o2.key.clone();
        let cancelled_bg = Arc::clone(&cancelled);
        let queued = std::thread::spawn(move || {
            sec_queued.redeem_cancellable(&key2_again, "Ghost", &cancelled_bg)
        });

        // Release the slow write and verify the original redeem succeeds while the cancelled one aborts.
        release_tx.send(()).unwrap();
        sec.set_before_save_hook(None);

        let dev2 = writer.join().unwrap().unwrap();
        assert!(matches!(queued.join().unwrap(), Err(RedeemError::Aborted)));
        assert_eq!(sec.devices().len(), 2);
        assert_eq!(sec.verify(&dev2.token).unwrap().0, dev2.device.id);
    }

    #[test]
    fn cancel_before_publish_leaves_devices_file_untouched_and_cleans_temp() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());

        // Seed an existing device so we can prove `devices.json` itself is never replaced/rolled back.
        let o0 = sec.start_pairing().unwrap();
        let dev0 = sec.redeem(&o0.key, "Existing Phone").unwrap();
        let devices_path = sec.dir().join("devices.json");
        let orig_ino = std::fs::symlink_metadata(&devices_path).unwrap().ino();

        // Case 1: offer is cancelled after temp file is staged, right before publish.
        let offer = sec.start_pairing().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        sec.set_before_publish_hook(Some(Arc::new(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().recv();
        })));

        let sec_bg = Arc::clone(&sec);
        let key = offer.key.clone();
        let writer = std::thread::spawn(move || sec_bg.redeem(&key, "Cancelled Phone"));

        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Cancel runs without blocking on disk I/O.
        assert!(sec.cancel_pairing(&offer.id));
        sec.set_before_publish_hook(None);
        release_tx.send(()).unwrap();

        assert!(matches!(writer.join().unwrap(), Err(RedeemError::Expired)));
        assert_eq!(sec.devices().len(), 1);
        assert_eq!(sec.devices()[0].id, dev0.device.id);
        // `devices.json` was never renamed over (same inode) and no `.tmp-*` file leaked.
        assert_eq!(
            std::fs::symlink_metadata(&devices_path).unwrap().ino(),
            orig_ino
        );
        let leaked_tmps: Vec<_> = std::fs::read_dir(sec.dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leaked_tmps.is_empty(), "leaked temp files: {leaked_tmps:?}");

        // Case 2: client disconnect (`cancelled` flag) set after staging, before publish -> Aborted,
        // offer stays active, `devices.json` stays untouched.
        let offer2 = sec.start_pairing().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        sec.set_before_publish_hook(Some(Arc::new(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().recv();
        })));

        let sec_bg = Arc::clone(&sec);
        let key2 = offer2.key.clone();
        let cancelled_bg = Arc::clone(&cancelled);
        let writer2 = std::thread::spawn(move || {
            sec_bg.redeem_cancellable(&key2, "Disconnected Phone", &cancelled_bg)
        });

        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancelled.store(true, Ordering::Release);
        sec.set_before_publish_hook(None);
        release_tx.send(()).unwrap();

        assert!(matches!(writer2.join().unwrap(), Err(RedeemError::Aborted)));
        assert!(sec.has_active_pairing());
        assert_eq!(
            std::fs::symlink_metadata(&devices_path).unwrap().ino(),
            orig_ino
        );

        // Reopening from disk still contains only the original device.
        drop(sec);
        let reopened = LanSecurity::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(reopened.devices().len(), 1);
        assert_eq!(reopened.devices()[0].id, dev0.device.id);
    }
}
