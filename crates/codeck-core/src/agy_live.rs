//! Live detection for agy (Antigravity CLI) Sessions via open presence lock files.
//!
//! An agy Session is live while an `antigravity` process holds `<agy_root>/presence/<id>.lock`
//! open. Detection is strictly read-only and never opens or locks files under the agy data root:
//! - macOS: `proc_listallpids` + `proc_pidpath` to find `antigravity` processes, then
//!   `proc_pidinfo(PROC_PIDLISTFDS)` + `proc_pidfdinfo(PROC_PIDFDVNODEPATHINFO)` to list open
//!   vnode paths.
//! - Linux: `/proc/<pid>/exe` to find `antigravity` processes, then `/proc/<pid>/fd/*` symlinks
//!   to list open paths.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Returns the filesystem paths of open file descriptors held by `pid`.
///
/// Strictly read-only; returns an empty list if `pid` cannot be inspected (e.g. exited or owned
/// by another user).
#[cfg(target_os = "macos")]
pub fn open_files(pid: u32) -> Vec<PathBuf> {
    use std::mem::{size_of, zeroed};
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    // From macOS `<sys/proc_info.h>` (not exported by the `libc` crate).
    const PROC_PIDFDVNODEPATHINFO: libc::c_int = 2;

    #[repr(C)]
    struct ProcFileInfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: libc::off_t,
        fi_type: i32,
        fi_guardflags: u32,
    }

    #[repr(C)]
    struct VnodeFdInfoWithPath {
        pfi: ProcFileInfo,
        pvip: libc::vnode_info_path,
    }

    let Ok(pid) = libc::c_int::try_from(pid) else {
        return Vec::new();
    };
    let needed = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let fd_size = size_of::<libc::proc_fdinfo>();
    let cap = (needed as usize / fd_size).saturating_add(16);
    let Ok(buf_bytes) = libc::c_int::try_from(cap.saturating_mul(fd_size)) else {
        return Vec::new();
    };
    // SAFETY: `proc_fdinfo` is plain old data; `proc_pidinfo` writes at most `buf_bytes`.
    let mut fds: Vec<libc::proc_fdinfo> = vec![unsafe { zeroed() }; cap];
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            buf_bytes,
        )
    };
    if n <= 0 {
        return Vec::new();
    }
    let count = (n as usize / fd_size).min(fds.len());
    let info_size = size_of::<VnodeFdInfoWithPath>() as libc::c_int;
    let mut out = Vec::new();
    for fd in &fds[..count] {
        if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
            continue;
        }
        // SAFETY: `VnodeFdInfoWithPath` is plain old data; the kernel fills `info_size` bytes.
        let mut info: VnodeFdInfoWithPath = unsafe { zeroed() };
        let got = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd.proc_fd,
                PROC_PIDFDVNODEPATHINFO,
                (&mut info as *mut VnodeFdInfoWithPath).cast(),
                info_size,
            )
        };
        if got != info_size {
            continue;
        }
        // SAFETY: `vip_path` is a fixed-size `[[c_char; 32]; 32]` (`MAXPATHLEN = 1024`) byte buffer.
        let raw = unsafe {
            std::slice::from_raw_parts(
                info.pvip.vip_path.as_ptr().cast::<u8>(),
                std::mem::size_of_val(&info.pvip.vip_path),
            )
        };
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if end > 0 {
            out.push(PathBuf::from(OsStr::from_bytes(&raw[..end])));
        }
    }
    out
}

/// Returns the filesystem paths of open file descriptors held by `pid` via `/proc/<pid>/fd`.
#[cfg(target_os = "linux")]
pub fn open_files(pid: u32) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path()) {
            out.push(target);
        }
    }
    out
}

/// Fallback for unsupported platforms: reports no open files.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn open_files(_pid: u32) -> Vec<PathBuf> {
    Vec::new()
}

/// Returns `session_id -> pid` for every `<agy_root>/presence/<id>.lock` currently held open by
/// a running `antigravity` process.
pub fn presence_holders(agy_root: &Path) -> BTreeMap<String, u32> {
    if !transcript::agy::presence_dir(agy_root).is_dir() {
        return BTreeMap::new();
    }
    let real_root = agy_root.canonicalize().ok();
    let mut out = BTreeMap::new();
    for pid in antigravity_pids() {
        for path in open_files(pid) {
            let sid = transcript::agy::session_of_presence(agy_root, &path).or_else(|| {
                real_root
                    .as_deref()
                    .and_then(|r| transcript::agy::session_of_presence(r, &path))
            });
            if let Some(sid) = sid {
                out.entry(sid).or_insert(pid);
            }
        }
    }
    out
}

fn is_antigravity_exe(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    name == "antigravity" || name == "antigravity (deleted)"
}

#[cfg(target_os = "macos")]
fn antigravity_pids() -> Vec<u32> {
    use std::mem::size_of;
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    let count = unsafe { libc::proc_listallpids(ptr::null_mut(), 0) };
    if count <= 0 {
        tracing::warn!(
            "proc_listallpids failed: {}",
            std::io::Error::last_os_error()
        );
        return Vec::new();
    }
    let cap = (count as usize).saturating_add(64);
    let Ok(buf_bytes) = libc::c_int::try_from(cap.saturating_mul(size_of::<libc::c_int>())) else {
        return Vec::new();
    };
    let mut pids: Vec<libc::c_int> = vec![0; cap];
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), buf_bytes) };
    if n <= 0 {
        tracing::warn!(
            "proc_listallpids failed: {}",
            std::io::Error::last_os_error()
        );
        return Vec::new();
    }
    let used = (n as usize).min(pids.len());
    let mut path_buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let mut out = Vec::new();
    for &pid in &pids[..used] {
        if pid <= 0 {
            continue;
        }
        let len =
            unsafe { libc::proc_pidpath(pid, path_buf.as_mut_ptr().cast(), path_buf.len() as u32) };
        if len <= 0 {
            continue;
        }
        let len = (len as usize).min(path_buf.len());
        let exe = Path::new(OsStr::from_bytes(&path_buf[..len]));
        if is_antigravity_exe(exe) {
            out.push(pid as u32);
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn antigravity_pids() -> Vec<u32> {
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("read /proc failed: {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Ok(exe) = std::fs::read_link(entry.path().join("exe"))
            && is_antigravity_exe(&exe)
        {
            out.push(pid);
        }
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn antigravity_pids() -> Vec<u32> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn open_files_reports_file_held_open_by_current_process() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("held-presence.lock");
        std::fs::write(&file_path, b"").unwrap();
        let canonical = file_path.canonicalize().unwrap();

        let held = File::open(&file_path).unwrap();
        let files = open_files(std::process::id());
        assert!(
            files.iter().any(|p| p == &canonical || p == &file_path),
            "expected {} in {} open files",
            canonical.display(),
            files.len()
        );

        drop(held);
        let files_after = open_files(std::process::id());
        assert!(
            !files_after
                .iter()
                .any(|p| p == &canonical || p == &file_path),
            "closed file should no longer appear in open_files"
        );
    }

    #[test]
    fn matches_antigravity_executable_names() {
        assert!(is_antigravity_exe(Path::new(
            "/opt/homebrew/Caskroom/antigravity-cli/1.5.18/antigravity"
        )));
        assert!(is_antigravity_exe(Path::new(
            "/usr/local/bin/antigravity (deleted)"
        )));
        assert!(!is_antigravity_exe(Path::new("/usr/bin/agy")));
        assert!(!is_antigravity_exe(Path::new(
            "/usr/local/bin/antigravity-helper"
        )));
    }
}
