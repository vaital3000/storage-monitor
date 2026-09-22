//! `storage-monitor modules`: the cleanup modules of this build, and what one of them finds.
//!
//! Both subcommands only look. Cleaning from the command line is `storage-monitor clean`,
//! which arrives with the first real module (phase 3). The one write either can cause is the
//! demo module's first discovery, which seeds its sandbox in the data dir.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use storage_monitor_core::module::{Availability, Context, Item, Level, Module};
use storage_monitor_core::paths;
use storage_monitor_core::system::RealSystem;
use storage_monitor_modules::registry;

use crate::quiet_on_broken_pipe;
use crate::report::human_bytes;

/// One module as `modules list` reports it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModuleReport {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    tools: &'static [&'static str],
    available: bool,
    /// Why it cannot run here, when it cannot.
    reason: Option<String>,
}

/// What `modules run` reports.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunReport<'a> {
    module: &'static str,
    items: &'a [Item],
}

/// Where a module looks: the machine's home and the app's data dir, which
/// `STORAGE_MONITOR_DATA_DIR` moves.
struct Places {
    home: PathBuf,
    data_dir: PathBuf,
}

impl Places {
    fn of_this_machine() -> Result<Self, String> {
        Ok(Self {
            home: paths::home_dir().ok_or("cannot determine the home folder")?,
            data_dir: paths::data_dir(),
        })
    }

    fn context<'a>(&'a self, sys: &'a RealSystem) -> Context<'a> {
        Context {
            sys,
            home: &self.home,
            data_dir: &self.data_dir,
        }
    }
}

pub fn list(json: bool) -> Result<ExitCode, String> {
    let places = Places::of_this_machine()?;
    let sys = RealSystem;
    let ctx = places.context(&sys);
    let reports: Vec<ModuleReport> = registry()
        .iter()
        .map(|module| {
            let descriptor = module.descriptor();
            let reason = match module.availability(&ctx) {
                Availability::Available => None,
                Availability::Unavailable { reason } => Some(reason),
            };
            ModuleReport {
                id: descriptor.id,
                name: descriptor.name,
                description: descriptor.description,
                tools: descriptor.tools,
                available: reason.is_none(),
                reason,
            }
        })
        .collect();
    if reports.is_empty() && !json {
        // On stderr, so that `modules list | wc -l` still counts modules.
        eprintln!("This build ships no cleanup modules.");
    }
    print(json, &reports, |out| {
        for report in &reports {
            let status = match &report.reason {
                None => "available".to_owned(),
                Some(reason) => format!("unavailable: {reason}"),
            };
            writeln!(out, "{}\t{}\t{status}", report.id, report.name)?;
            writeln!(out, "\t{}", report.description)?;
        }
        Ok(())
    })?;
    Ok(ExitCode::SUCCESS)
}

pub fn run(id: &str, json: bool) -> Result<ExitCode, String> {
    let modules = registry();
    let module = find(&modules, id)?;
    let places = Places::of_this_machine()?;
    let sys = RealSystem;
    let ctx = places.context(&sys);
    if let Availability::Unavailable { reason } = module.availability(&ctx) {
        return Err(format!("the {id} module cannot run here: {reason}"));
    }
    let items = module
        .discover(&ctx)
        .map_err(|err| format!("the {id} module could not look around: {err}"))?;
    let report = RunReport {
        module: module.descriptor().id,
        items: &items,
    };
    print(json, &report, |out| {
        for item in &items {
            let size = if item.size.estimated {
                format!("~{}", human_bytes(item.size.bytes))
            } else {
                human_bytes(item.size.bytes)
            };
            writeln!(
                out,
                "{:<6}  {size:>9}  {}",
                level(item.verdict.level),
                item.title
            )?;
            for reason in &item.verdict.reasons {
                writeln!(out, "{:>19}{}", "", reason.text)?;
            }
        }
        Ok(())
    })?;
    Ok(ExitCode::SUCCESS)
}

/// The module with this id, or an error that names the ones there are.
fn find<'a>(modules: &'a [Box<dyn Module>], id: &str) -> Result<&'a dyn Module, String> {
    if let Some(module) = modules.iter().find(|module| module.descriptor().id == id) {
        return Ok(module.as_ref());
    }
    let known: Vec<&str> = modules
        .iter()
        .map(|module| module.descriptor().id)
        .collect();
    Err(if known.is_empty() {
        format!("unknown module {id}: this build ships no cleanup modules")
    } else {
        format!("unknown module {id}; this build has: {}", known.join(", "))
    })
}

fn level(level: Level) -> &'static str {
    match level {
        Level::Safe => "safe",
        Level::Review => "review",
        Level::Keep => "keep",
    }
}

/// Writes `value` as JSON or through `text`, to a locked stdout, and treats a closed pipe as
/// the end of the conversation rather than an error.
fn print<T: Serialize>(
    json: bool,
    value: &T,
    text: impl FnOnce(&mut io::StdoutLock<'static>) -> io::Result<()>,
) -> Result<(), String> {
    let mut out = io::stdout().lock();
    let written = if json {
        serde_json::to_writer_pretty(&mut out, value)
            .map_err(io::Error::from)
            .and_then(|()| writeln!(out))
    } else {
        text(&mut out)
    };
    match written {
        Ok(()) => out.flush().or_else(quiet_on_broken_pipe),
        Err(err) => quiet_on_broken_pipe(err),
    }
    .map_err(|err| err.to_string())
}
