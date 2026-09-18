use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use storage_monitor_core::app_info;

use crate::scan_cmd::ScanArgs;

mod report;
mod scan_cmd;

#[derive(Parser)]
#[command(
    name = "storage-monitor",
    version,
    about = "Disk space analyzer with cleanup modules"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print application metadata
    Info {
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },
    /// Scan a folder (default: the home folder) and report where the space goes
    Scan {
        /// Folder to scan
        root: Option<PathBuf>,
        /// Print as JSON
        #[arg(long)]
        json: bool,
        /// Depth of the reported tree
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Children per directory in the report, largest first
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Persist a snapshot and report growth against the previous one
        #[arg(long)]
        save: bool,
        /// Smallest file kept in the snapshot, in bytes
        #[arg(long, default_value_t = storage_monitor_core::snapshot::DEFAULT_FILE_THRESHOLD)]
        threshold: u64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Info { json } => print_info(json).map(|()| ExitCode::SUCCESS),
        Command::Scan {
            root,
            json,
            depth,
            top,
            save,
            threshold,
        } => scan_cmd::run(ScanArgs {
            root,
            json,
            depth,
            top,
            save,
            threshold,
        }),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

/// Downstream closed the pipe (for example `... --json | head`): not an error.
fn quiet_on_broken_pipe(err: io::Error) -> io::Result<()> {
    if err.kind() == io::ErrorKind::BrokenPipe {
        Ok(())
    } else {
        Err(err)
    }
}

fn print_info(json: bool) -> Result<(), String> {
    let info = app_info();
    let mut out = io::stdout().lock();
    let written = if json {
        serde_json::to_writer_pretty(&mut out, &info)
            .map_err(io::Error::from)
            .and_then(|()| writeln!(out))
    } else {
        writeln!(out, "{} {}", info.name, info.version)
    };
    match written {
        Ok(()) => out.flush().or_else(quiet_on_broken_pipe),
        Err(err) => quiet_on_broken_pipe(err),
    }
    .map_err(|e| e.to_string())
}
