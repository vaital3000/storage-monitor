//! `storage-monitor scan`: scan a folder and report where the space goes.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use storage_monitor_core::disk::disk_usage;
use storage_monitor_core::paths;
use storage_monitor_core::scan::{NodeKind, ScanOptions, ScanProgress, ScanResult, scan};
use storage_monitor_core::snapshot::{
    DEFAULT_KEEP, Delta, Snapshot, SnapshotStore, StoreError, deltas, top_growers,
};

use crate::quiet_on_broken_pipe;
use crate::report::{ScanReport, human_bytes, report_tree};

/// Exit code of a scan that was cancelled (not reachable from the command line yet).
const EXIT_CANCELLED: u8 = 130;
/// Exit code when `--save` could not store the snapshot; the report is printed anyway.
const EXIT_SAVE_FAILED: u8 = 1;
/// Snapshots kept in the store, across all roots.
/// Growers listed in the report.
const GROWERS_REPORTED: usize = 20;
/// Interval of the progress lines on stderr.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

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

    let mut save_failed = false;
    let (snapshot_path, growers) = match args.save.then(|| save_snapshot(&result, args.threshold)) {
        Some(Ok((path, growers))) => (Some(path), growers),
        Some(Err(err)) => {
            // The scan itself succeeded: print the report and let the exit code tell.
            eprintln!("warning: could not save the snapshot: {err}");
            save_failed = true;
            (None, Vec::new())
        }
        None => (None, Vec::new()),
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
    } else if save_failed {
        ExitCode::from(EXIT_SAVE_FAILED)
    } else {
        ExitCode::SUCCESS
    })
}

/// The absolute form of `root` without trailing slashes or `.` components, so the key of
/// a snapshot depends neither on the working directory nor on how the path was typed.
/// Symlinks are kept as given: the scanner follows a symlinked root itself, and the
/// desktop app does not canonicalize either, so both key the same folder the same way.
/// A missing root is left to the scanner to report.
fn resolve_root(root: PathBuf) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(&root)
        .map_err(|e| format!("cannot resolve {}: {e}", root.display()))?;
    Ok(absolute.components().collect())
}

/// Stores a snapshot of `result` and returns its path with the growers since the previous
/// snapshot of the same root.
fn save_snapshot(result: &ScanResult, threshold: u64) -> Result<(PathBuf, Vec<Delta>), StoreError> {
    let store = SnapshotStore::new(paths::snapshots_dir());
    let previous = store.latest_for(&result.root)?;
    let current = Snapshot::from_result(result, threshold);
    let path = store.save(&current)?;
    store.prune(DEFAULT_KEEP)?;
    let growers = previous
        .map(|p| top_growers(&deltas(&p, &current), GROWERS_REPORTED))
        .unwrap_or_default();
    Ok((path, growers))
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

/// A thread that prints the live counters while the scan runs. [`Ticker::stop`] wakes it
/// at once, so the scan never waits for an interval to pass, and a ticker stopped before
/// its first line prints nothing.
struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Ticker {
    /// A ticker that prints nothing: stderr is not a terminal.
    fn silent() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Prints one `scanning: ...` line to `out` every `interval`, over the previous one,
    /// and clears the line when stopped.
    fn spawn(
        progress: Arc<ScanProgress>,
        interval: Duration,
        mut out: impl Write + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut printed = false;
            loop {
                std::thread::park_timeout(interval);
                if flag.load(Ordering::Acquire) {
                    break;
                }
                let p = progress.snapshot();
                // Progress lines are best effort: a failed write is no reason to stop.
                let _ = write!(
                    out,
                    "\r\x1b[2Kscanning: {} files, {}  {}",
                    p.files,
                    human_bytes(p.bytes),
                    shorten(&p.current_path, 60)
                );
                let _ = out.flush();
                printed = true;
            }
            if printed {
                let _ = write!(out, "\r\x1b[2K");
                let _ = out.flush();
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

fn spawn_progress_printer(progress: Arc<ScanProgress>) -> Ticker {
    if io::stderr().is_terminal() {
        Ticker::spawn(progress, PROGRESS_INTERVAL, io::stderr())
    } else {
        Ticker::silent()
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
    use std::fs;
    use std::sync::Mutex;
    use std::time::Instant;

    use super::*;

    /// A shared buffer the ticker writes to instead of stderr.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Sink {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    #[test]
    fn shorten_keeps_the_tail_of_long_paths() {
        assert_eq!(shorten("/short", 10), "/short");
        assert_eq!(shorten("/a/very/long/path", 8), "…ng/path");
        assert_eq!(shorten("/ünïcödé/path", 6), "…/path");
    }

    #[test]
    fn a_stopped_ticker_returns_at_once_and_prints_nothing() {
        let sink = Sink::default();
        let ticker = Ticker::spawn(
            Arc::new(ScanProgress::default()),
            Duration::from_millis(500),
            sink.clone(),
        );
        let started = Instant::now();
        ticker.stop();
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "stop waited for the tick: {:?}",
            started.elapsed()
        );
        assert_eq!(
            sink.text(),
            "",
            "no line for a scan that ended before a tick"
        );
    }

    #[test]
    fn the_ticker_prints_the_counters_and_clears_the_line_when_stopped() {
        let sink = Sink::default();
        let ticker = Ticker::spawn(
            Arc::new(ScanProgress::default()),
            Duration::from_millis(1),
            sink.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sink.text().contains("scanning:") {
            assert!(Instant::now() < deadline, "no progress line was printed");
            std::thread::sleep(Duration::from_millis(1));
        }
        ticker.stop();
        let text = sink.text();
        assert!(
            text.contains("\r\x1b[2Kscanning: 0 files, 0 B  "),
            "{text:?}"
        );
        assert!(text.ends_with("\r\x1b[2K"), "the line is cleared: {text:?}");
    }

    #[test]
    fn resolve_root_is_absolute_without_trailing_slashes_or_dots() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolve_root(PathBuf::from(".")).unwrap(), cwd);
        assert_eq!(
            resolve_root(PathBuf::from("./src/")).unwrap(),
            cwd.join("src")
        );
        assert_eq!(
            resolve_root(PathBuf::from("/tmp/")).unwrap(),
            PathBuf::from("/tmp")
        );
        assert_eq!(
            resolve_root(PathBuf::from("/tmp/./x/.")).unwrap(),
            PathBuf::from("/tmp/x")
        );
    }

    #[test]
    fn resolve_root_keeps_a_symlinked_root_as_given() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("real")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path().join("real"), &link).unwrap();
        assert_eq!(resolve_root(link.clone()).unwrap(), link);
    }
}
