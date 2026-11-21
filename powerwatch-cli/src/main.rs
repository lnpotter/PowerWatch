mod alert_config;
mod format;
mod history;

use clap::{Parser, Subcommand};
use powerwatch_core::sampler::{Sampler, Snapshot};
use powerwatch_core::storage::Storage;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "powerwatch")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long)]
    watch: bool,

    #[arg(long, default_value_t = 2)]
    interval: u64,

    #[arg(long)]
    json: bool,

    #[arg(long)]
    log: bool,

    #[arg(long)]
    duration: Option<String>,

    #[arg(long)]
    alert_component: Option<String>,

    #[arg(long)]
    alert_above: Option<f64>,

    #[arg(long)]
    alert_for: Option<String>,

    #[arg(long)]
    alert_run: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    History {
        #[arg(long)]
        since: String,
    },
}

fn history_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".local/share/powerwatch/history.db")
}

fn render(snapshot: &Snapshot, as_json: bool) -> String {
    if as_json {
        powerwatch_core::json_snapshot::format_snapshot_json(snapshot)
    } else {
        format::format_snapshot_table(snapshot)
    }
}

fn log_for(sampler: &mut Sampler, storage: &Storage, log_duration: Duration) -> usize {
    let deadline = Instant::now() + log_duration;
    let mut recorded = 0;

    while Instant::now() < deadline {
        let snapshot = sampler.sample_all();

        if let Ok(inserted) = storage.insert_snapshot(&snapshot) {
            recorded += inserted;
        }

        thread::sleep(Duration::from_secs(1));
    }

    recorded
}

fn run_history(since_text: &str) {
    let lookback = match powerwatch_core::duration::parse_duration(since_text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let db_path = history_db_path();
    let storage = match Storage::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "failed to open history database at {}: {e:?} (have you run --log yet?)",
                db_path.display()
            );
            std::process::exit(1);
        }
    };

    let cutoff = chrono::Utc::now()
        - chrono::Duration::from_std(lookback).unwrap_or(chrono::Duration::zero());

    let readings = match storage.readings_since(cutoff) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to read history: {e:?}");
            std::process::exit(1);
        }
    };

    let summaries = history::aggregate_by_component(&readings);
    println!("{}", history::format_history_table(&summaries));
}

fn main() {
    let args = Args::parse();

    if let Some(Command::History { since }) = args.command.as_ref() {
        run_history(since);
        return;
    }

    let mut sampler = powerwatch_core::runtime::build_default_sampler();

    if sampler.is_empty() {
        eprintln!("no power sensors available on this machine - nothing to show");
        std::process::exit(1);
    }

    if args.log {
        let Some(duration_text) = args.duration.as_deref() else {
            eprintln!("--log requires --duration, e.g. --log --duration 60s");
            std::process::exit(1);
        };
        let log_duration = match powerwatch_core::duration::parse_duration(duration_text) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };

        let db_path = history_db_path();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let storage = match Storage::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "failed to open history database at {}: {e:?}",
                    db_path.display()
                );
                std::process::exit(1);
            }
        };

        let _ = storage.prune_expired();

        println!("logging to {} for {duration_text}...", db_path.display());
        let recorded = log_for(&mut sampler, &storage, log_duration);
        println!("done - recorded {recorded} readings");
        return;
    }

    let alert_setup = match alert_config::resolve_alert_config(
        args.alert_component.as_deref(),
        args.alert_above,
        args.alert_for.as_deref(),
        args.alert_run.as_deref(),
        args.watch,
    ) {
        Ok(setup) => setup,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let (mut alert_evaluator, alert_hook) = match alert_setup {
        Some((rule, hook)) => (
            Some(powerwatch_core::alerts::AlertEvaluator::new(rule)),
            hook,
        ),
        None => (None, None),
    };

    if args.watch {
        loop {
            let snapshot = sampler.sample_all();

            if let Some(evaluator) = alert_evaluator.as_mut() {
                if evaluator.evaluate_at(&snapshot, Instant::now()) {
                    eprintln!("\n🔔 ALERT: threshold sustained - firing");
                    if let Some(command) = &alert_hook {
                        if let Err(e) = powerwatch_core::hook::run_hook(command) {
                            eprintln!("failed to run alert hook: {e:?}");
                        }
                    }
                }
            }

            if !args.json {
                print!("\x1B[2J\x1B[1;1H"); // clear the screen between updates
            }
            println!("{}", render(&snapshot, args.json));
            if let Some(evaluator) = alert_evaluator.as_ref() {
                if evaluator.is_active() {
                    println!("🔔 alert active");
                }
            }
            thread::sleep(Duration::from_secs(args.interval.max(1)));
        }
    } else {
        sampler.sample_all();
        thread::sleep(Duration::from_millis(300));

        let snapshot = sampler.sample_all();
        println!("{}", render(&snapshot, args.json));
    }
}
