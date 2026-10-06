//! Runtime directory for the collector socket and lock file.

use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// `$NYSM_RUNTIME_DIR`, else `$XDG_RUNTIME_DIR/nysm`, else `/tmp/nysm-<uid>`.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NYSM_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join(nysm_core::brand::COMMAND_NAME);
    }
    // SAFETY: geteuid cannot fail.
    let uid = unsafe { libc::geteuid() };
    std::env::temp_dir().join(format!("{}-{uid}", nysm_core::brand::COMMAND_NAME))
}

pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join("collector.sock")
}

/// Unix socket paths are limited (`sun_path`, 108 bytes on Linux, 104 on
/// macOS). Fail early with an actionable message.
pub fn check_socket_path(p: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    const MAX: usize = 103;
    let n = p.as_os_str().as_bytes().len();
    if n > MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "socket path {} is {n} bytes (max {MAX}); set NYSM_RUNTIME_DIR to a shorter directory",
                p.display()
            ),
        ));
    }
    Ok(())
}

pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("collector.lock")
}

/// Create `dir` with mode 0700 if missing, and refuse to use it unless it
/// is a directory owned by us with no group/other permissions.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
    {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: geteuid cannot fail.
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() {
        return Err(io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if meta.uid() != me {
        return Err(io::Error::other(format!(
            "{} is owned by uid {}, not us ({me})",
            dir.display(),
            meta.uid()
        )));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "{} is accessible by other users (mode {:o}); refusing to use it",
            dir.display(),
            meta.permissions().mode() & 0o777
        )));
    }
    Ok(())
}

/// Uid of the peer on a connected Unix socket.
pub fn peer_uid(stream: &std::os::unix::net::UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: valid fd and out-pointers of the right size.
        let r = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: valid fd and out-pointers.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dir_is_created_and_checked() {
        let base = std::env::temp_dir().join(format!("nysm-paths-{}", std::process::id()));
        let d = base.join("rt");
        ensure_private_dir(&d).unwrap();
        assert_eq!(
            std::fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            ensure_private_dir(&d)
                .unwrap_err()
                .to_string()
                .contains("refusing")
        );
        let f = base.join("file");
        std::fs::write(&f, "").unwrap();
        assert!(ensure_private_dir(&f).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
