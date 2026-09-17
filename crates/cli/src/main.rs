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
    match cli.command {
        Command::Info { json } => {
            let info = app_info();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&info).expect("AppInfo is serializable")
                );
            } else {
                println!("{} {}", info.name, info.version);
            }
        }
    }
}
