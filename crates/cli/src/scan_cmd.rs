//! `storage-monitor scan`: scan a folder and report where the space goes.

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use storage_monitor_core::disk::disk_usage;
use storage_monitor_core::paths;
use storage_monitor_core::scan::{NodeKind, ScanOptions, ScanProgress, scan};
use storage_monitor_core::snapshot::{Snapshot, SnapshotStore, deltas, top_growers};

use crate::quiet_on_broken_pipe;
use crate::report::{ScanReport, human_bytes, report_tree};

/// Exit code of a scan that was cancelled (not reachable from the command line yet).
const EXIT_CANCELLED: u8 = 130;

pub struct ScanArgs {
    pub root: Option<PathBuf>,
    pub json: bool,
    pub depth: usize,
    pub top: usize,
    pub save: bool,
    pub threshold: u64,
}

pub fn run(args: ScanArgs) -> Result<ExitCode, String> {
    let root = match args.root {
        Some(root) => resolve_root(root)?,
        None => paths::home_dir().ok_or("cannot determine the home folder")?,
    };
    let progress = Arc::new(ScanProgress::default());
    let ticker = spawn_progress_printer(Arc::clone(&progress));
    let result = scan(&ScanOptions::new(root), &progress).map_err(|e| e.to_string());
    ticker.stop();
    let result = result?;

    let (snapshot_path, growers) = if args.save {
        let store = SnapshotStore::new(paths::snapshots_dir());
        let previous = store.latest_for(&result.root).map_err(|e| e.to_string())?;
        let current = Snapshot::from_result(&result, args.threshold);
        let path = store.save(&current).map_err(|e| e.to_string())?;
        store.prune(10).map_err(|e| e.to_string())?;
        let growers = previous
            .map(|p| top_growers(&deltas(&p, &current), 20))
            .unwrap_or_default();
        (Some(path), growers)
    } else {
        (None, Vec::new())
    };

    let report = ScanReport {
        root: result.root.clone(),
        started_at: result.started_at,
        duration_ms: result.duration_ms,
        stats: result.stats.clone(),
        cancelled: result.cancelled,
        disk: disk_usage(&result.root).ok(),
        tree: report_tree(&result, args.depth, args.top),
        snapshot: snapshot_path,
        top_growers: growers,
    };

    let mut out = io::stdout().lock();
    let written = if args.json {
        serde_json::to_writer_pretty(&mut out, &report)
            .map_err(io::Error::from)
            .and_then(|()| writeln!(out))
    } else {
        write_text(&mut out, &report)
    };
    match written {
        Ok(()) => out.flush().or_else(quiet_on_broken_pipe),
        Err(err) => quiet_on_broken_pipe(err),
    }
    .map_err(|e| e.to_string())?;
    Ok(if report.cancelled {
        ExitCode::from(EXIT_CANCELLED)
    } else {
        ExitCode::SUCCESS
    })
}

/// The absolute form of `root`, so snapshot paths do not depend on the working directory.
/// A root that is itself a symlink (`/tmp` on macOS) is resolved, because the scanner never
/// follows symlinks; a missing root is left to the scanner to report.
fn resolve_root(root: PathBuf) -> Result<PathBuf, String> {
    let cannot = |e: io::Error| format!("cannot resolve {}: {e}", root.display());
    let absolute = std::path::absolute(&root).map_err(cannot)?;
    match fs::symlink_metadata(&absolute) {
        Ok(meta) if meta.is_symlink() => fs::canonicalize(&absolute).map_err(cannot),
        _ => Ok(absolute),
    }
}

fn write_text(out: &mut impl Write, report: &ScanReport) -> io::Result<()> {
    writeln!(
        out,
        "{}  {}  ({} files, {} dirs, {} errors, {:.1}s)",
        report.root.display(),
        human_bytes(report.tree.size),
        report.stats.files,
        report.stats.dirs,
        report.stats.errors,
        report.duration_ms as f64 / 1000.0
    )?;
    if let Some(disk) = &report.disk {
        writeln!(
            out,
            "volume: {} used of {} ({} available)",
            human_bytes(disk.used),
            human_bytes(disk.total),
            human_bytes(disk.available)
        )?;
    }
    let total = report.tree.size.max(1) as f64;
    for child in &report.tree.children {
        let suffix = if child.kind == NodeKind::Dir { "/" } else { "" };
        writeln!(
            out,
            "{:>10}  {:5.1}%  {}{}",
            human_bytes(child.size),
            child.size as f64 * 100.0 / total,
            child.name,
            suffix
        )?;
    }
    if !report.top_growers.is_empty() {
        writeln!(out, "\ngrew since the previous snapshot:")?;
        for g in &report.top_growers {
            writeln!(
                out,
                "{:>10}  {}",
                format!("+{}", human_bytes(g.delta.max(0) as u64)),
                g.path
            )?;
        }
    }
    Ok(())
}

/// Prints the live counters to stderr every 500 ms while the scan runs, only when stderr
/// is a terminal.
struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Ticker {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_progress_printer(progress: Arc<ScanProgress>) -> Ticker {
    let stop = Arc::new(AtomicBool::new(false));
    if !io::stderr().is_terminal() {
        return Ticker { stop, handle: None };
    }
    let flag = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut printed = false;
        while !flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(500));
            let p = progress.snapshot();
            eprint!(
                "\r\x1b[2Kscanning: {} files, {}  {}",
                p.files,
                human_bytes(p.bytes),
                shorten(&p.current_path, 60)
            );
            printed = true;
        }
        if printed {
            eprint!("\r\x1b[2K");
        }
    });
    Ticker {
        stop,
        handle: Some(handle),
    }
}

fn shorten(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max {
        path.to_owned()
    } else {
        format!(
            "…{}",
            path.chars().skip(count - max + 1).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_keeps_the_tail_of_long_paths() {
        assert_eq!(shorten("/short", 10), "/short");
        assert_eq!(shorten("/a/very/long/path", 8), "…ng/path");
        assert_eq!(shorten("/ünïcödé/path", 6), "…/path");
    }
}
