use std::io::{self, Write};

use clap::{Parser, Subcommand};
use storage_monitor_core::app_info;

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
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Info { json } => print_info(json),
    };
    if let Err(err) = result {
        // Downstream closed the pipe (for example `... --json | head`); exit quietly.
        if err.kind() == io::ErrorKind::BrokenPipe {
            return;
        }
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn print_info(json: bool) -> io::Result<()> {
    let info = app_info();
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &info)?;
        writeln!(out)?;
    } else {
        writeln!(out, "{} {}", info.name, info.version)?;
    }
    out.flush()
}
