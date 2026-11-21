mod app;
mod state;
mod suggestions;

use clap::Parser;
use powerwatch_core::runtime::build_default_sampler;
use powerwatch_core::storage::Storage;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 3000)]
    port: u16,

    #[arg(long)]
    log: bool,
}

fn history_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".local/share/powerwatch/history.db")
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let sampler = build_default_sampler();
    if sampler.is_empty() {
        eprintln!("no power sensors available on this machine - nothing to show");
        std::process::exit(1);
    }

    let db_path = history_db_path();
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let storage = Storage::open(&db_path).ok();
    if args.log {
        if let Some(storage) = &storage {
            let _ = storage.prune_expired();
        }
    }
    if args.log && storage.is_none() {
        eprintln!(
            "warning: --log was passed but the history database at {} couldn't be opened",
            db_path.display()
        );
    }

    let app_state = state::start_sampling_loop(sampler, SAMPLE_INTERVAL, storage, args.log);

    let router = app::build_router(app_state);
    let addr = format!("127.0.0.1:{}", args.port);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    println!("powerwatch-web listening on http://{addr}");
    if args.log {
        println!("logging continuously to {}", db_path.display());
    }
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
