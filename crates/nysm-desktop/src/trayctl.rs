//! Start/stop the tray indicator and its login autostart from the app.
//! Only this user's `nysm-tray` processes are touched; autostart is a
//! per-user `.desktop` file that exists only while the user enables it.

use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BIN: &str = "nysm-tray";

/// The tray binary: next to this app, else on PATH.
fn tray_command() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(BIN)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(BIN))
}

/// PIDs of this user's running tray processes.
pub fn running() -> Vec<i32> {
    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            let exe = std::fs::read_link(e.path().join("exe")).ok()?;
            let name = exe.file_name()?.to_string_lossy().into_owned();
            let ours = e.metadata().ok()?.uid() == me;
            (ours && name.trim_end_matches(" (deleted)") == BIN).then_some(pid)
        })
        .collect()
}

pub fn start() -> Result<(), String> {
    if !running().is_empty() {
        return Ok(());
    }
    let cmd = tray_command();
    let mut child = Command::new(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Own process group: not stopped by signals aimed at the app.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", cmd.display()))?;
    // Reap it whenever it exits, so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn stop() {
    for pid in running() {
        // SAFETY: plain signal to a process verified above to be this
        // user's nysm-tray.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

fn autostart_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join("nysm-tray.desktop"))
}

pub fn autostart_enabled() -> bool {
    autostart_file().is_some_and(|p| p.exists())
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let path = autostart_file().ok_or("no config directory ($HOME is not set)")?;
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{}: {e}", path.display()))
            }
            _ => Ok(()),
        };
    }
    let exec = tray_command();
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Now You See Me (tray)\n\
         Comment=Live CPU, memory, network and disk in the top bar\n\
         Exec={}\n\
         Icon=utilities-system-monitor\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        exec.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, entry).map_err(|e| format!("{}: {e}", path.display()))
}
