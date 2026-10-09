//! `nysm disk usage PATH`: bounded, cancellable "what uses the space"
//! scan of one directory tree. Never automatic, never a full-disk crawl.
//!
//! Bounds: maximum entries, time limit, Ctrl-C. Stays on the starting
//! filesystem by default, never follows symlinks, counts allocated blocks
//! (st_blocks × 512) and hard-linked files once.

use std::collections::HashSet;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nysm_core::units;
use serde::Serialize;

use crate::{Ctx, exit, fmt};

static CANCEL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    CANCEL.store(true, Ordering::SeqCst);
}

#[derive(Serialize, Clone)]
struct Entry {
    path: String,
    bytes: u64,
    files: u64,
    dirs: u64,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    root: String,
    total_bytes: u64,
    files: u64,
    dirs: u64,
    /// Directories that could not be read (permission denied etc.).
    unreadable: u64,
    /// Entries on other filesystems that were skipped.
    other_filesystems: u64,
    entries_scanned: u64,
    elapsed_ms: u64,
    /// Why the scan stopped early, if it did; totals are then lower bounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<String>,
    children: Vec<Entry>,
}

pub struct Limits {
    pub max_entries: u64,
    pub timeout: Duration,
    pub one_file_system: bool,
}

struct Scan {
    limits: Limits,
    root_dev: u64,
    seen_links: HashSet<(u64, u64)>,
    entries: u64,
    unreadable: u64,
    other_fs: u64,
    start: Instant,
    stopped: Option<String>,
}

impl Scan {
    fn should_stop(&mut self) -> bool {
        if self.stopped.is_some() {
            return true;
        }
        if CANCEL.load(Ordering::SeqCst) {
            self.stopped = Some("interrupted (Ctrl-C)".into());
        } else if self.entries >= self.limits.max_entries {
            self.stopped = Some(format!("entry limit {} reached", self.limits.max_entries));
        } else if self.start.elapsed() >= self.limits.timeout {
            self.stopped = Some(format!(
                "time limit {} s reached",
                self.limits.timeout.as_secs()
            ));
        }
        self.stopped.is_some()
    }

    /// Size of one entry; directories are walked iteratively (no recursion
    /// depth limit concerns on deep trees).
    fn measure(&mut self, top: &Path) -> Entry {
        let mut e = Entry {
            path: top.display().to_string(),
            bytes: 0,
            files: 0,
            dirs: 0,
        };
        let mut stack = vec![top.to_path_buf()];
        while let Some(p) = stack.pop() {
            if self.should_stop() {
                break;
            }
            self.entries += 1;
            let Ok(m) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if self.limits.one_file_system && m.dev() != self.root_dev {
                self.other_fs += 1;
                continue;
            }
            if m.is_dir() {
                e.dirs += 1;
                e.bytes += m.blocks() * 512;
                match std::fs::read_dir(&p) {
                    Ok(rd) => stack.extend(rd.flatten().map(|d| d.path())),
                    Err(_) => self.unreadable += 1,
                }
            } else {
                if m.nlink() > 1 && !self.seen_links.insert((m.dev(), m.ino())) {
                    continue; // hard link already counted
                }
                e.files += 1;
                e.bytes += m.blocks() * 512;
            }
        }
        e
    }
}

fn scan(root: &Path, limits: Limits) -> io::Result<Report> {
    let meta = std::fs::symlink_metadata(root)?;
    let mut s = Scan {
        root_dev: meta.dev(),
        limits,
        seen_links: HashSet::new(),
        entries: 0,
        unreadable: 0,
        other_fs: 0,
        start: Instant::now(),
        stopped: None,
    };
    let mut children = Vec::new();
    let (mut files, mut dirs, mut total) = (0, 1, meta.blocks() * 512);
    if meta.is_dir() {
        let mut kids: Vec<PathBuf> = {
            let rd = std::fs::read_dir(root)?;
            rd.flatten().map(|d| d.path()).collect()
        };
        kids.sort();
        for k in kids {
            if s.should_stop() {
                break;
            }
            let e = s.measure(&k);
            total += e.bytes;
            files += e.files;
            dirs += e.dirs;
            children.push(e);
        }
    } else {
        files = 1;
        dirs = 0;
    }
    children.sort_by_key(|x| std::cmp::Reverse(x.bytes));
    Ok(Report {
        schema_version: nysm_core::SCHEMA_VERSION,
        root: root.display().to_string(),
        total_bytes: total,
        files,
        dirs,
        unreadable: s.unreadable,
        other_filesystems: s.other_fs,
        entries_scanned: s.entries,
        elapsed_ms: s.start.elapsed().as_millis() as u64,
        stopped: s.stopped,
        children,
    })
}

pub fn run(ctx: &Ctx, path: &Path, top: usize, limits: Limits, json: bool) -> io::Result<u8> {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
    }
    let mut report = match scan(path, limits) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("nysm: cannot scan {}: {e}", path.display());
            return Ok(if e.kind() == io::ErrorKind::NotFound {
                exit::NOT_FOUND
            } else {
                exit::FAILURE
            });
        }
    };
    report.children.truncate(top);
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &report)?;
        writeln!(out)?;
        return Ok(exit::OK);
    }
    let st = &ctx.style;
    let partial = if report.stopped.is_some() {
        " (at least)"
    } else {
        ""
    };
    writeln!(
        out,
        "{} {}{partial} in {} files, {} directories · scanned {} entries in {:.1} s",
        st.bold(&fmt::safe(&report.root)),
        units::bytes(report.total_bytes as f64),
        report.files,
        report.dirs,
        report.entries_scanned,
        report.elapsed_ms as f64 / 1000.0
    )?;
    for c in &report.children {
        let share = if report.total_bytes > 0 {
            c.bytes as f64 / report.total_bytes as f64 * 100.0
        } else {
            0.0
        };
        let name = Path::new(&c.path)
            .file_name()
            .map_or(c.path.clone(), |n| n.to_string_lossy().into_owned());
        writeln!(
            out,
            "  {:>9} {:>5.1}%  {}",
            units::bytes(c.bytes as f64),
            share,
            fmt::safe(&name)
        )?;
    }
    if let Some(why) = &report.stopped {
        writeln!(
            out,
            "{}",
            st.warn(&format!("stopped early: {why}; sizes are lower bounds"))
        )?;
    }
    if report.unreadable > 0 {
        writeln!(
            out,
            "{}",
            st.dim(&format!(
                "{} directories could not be read (not counted)",
                report.unreadable
            ))
        )?;
    }
    if report.other_filesystems > 0 {
        writeln!(
            out,
            "{}",
            st.dim(&format!(
                "{} entries on other filesystems skipped (use --cross-filesystems)",
                report.other_filesystems
            ))
        )?;
    }
    writeln!(
        out,
        "{}",
        st.dim("sizes are allocated disk space; hard links counted once; symlinks not followed")
    )?;
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_files_once_and_respects_entry_limit() {
        let d = std::env::temp_dir().join(format!("nysm-du-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("big/sub")).unwrap();
        std::fs::create_dir_all(d.join("small")).unwrap();
        std::fs::write(d.join("big/sub/a"), vec![1u8; 200_000]).unwrap();
        std::fs::write(d.join("small/b"), vec![1u8; 10]).unwrap();
        std::fs::hard_link(d.join("big/sub/a"), d.join("big/a-link")).unwrap();
        std::os::unix::fs::symlink("/", d.join("small/root-link")).unwrap();
        let lim = || Limits {
            max_entries: 1_000_000,
            timeout: Duration::from_secs(10),
            one_file_system: true,
        };
        let r = scan(&d, lim()).unwrap();
        assert!(r.stopped.is_none());
        assert_eq!(r.children[0].path, d.join("big").display().to_string());
        // The hard link is not double counted: big ≈ one 200 kB file + dirs.
        assert!(r.children[0].bytes < 2 * 200_000, "{}", r.children[0].bytes);
        assert_eq!(r.files, 3, "a, b and the symlink itself (not followed)");
        let r = scan(
            &d,
            Limits {
                max_entries: 2,
                ..lim()
            },
        )
        .unwrap();
        assert!(
            r.stopped
                .as_deref()
                .is_some_and(|s| s.contains("entry limit"))
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
