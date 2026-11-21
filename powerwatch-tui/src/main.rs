mod history;
mod render;
mod state;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use history::History;
use powerwatch_core::alerts::{resolve_alert_rule, AlertEvaluator};
use powerwatch_core::runtime::build_default_sampler;
use powerwatch_core::sampler::Sampler;
use powerwatch_core::storage::Storage;
use powerwatch_core::suggestions::{
    ActionKind, PowerProfile, Suggestion, SuggestionEngine, SuggestionRule,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    alert_component: Option<String>,
    #[arg(long)]
    alert_above: Option<f64>,
    #[arg(long)]
    alert_for: Option<String>,
    #[arg(long)]
    alert_run: Option<String>,
}

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const HISTORY_CAPACITY: usize = 60;

fn history_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".local/share/powerwatch/history.db")
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        default_hook(panic_info);
    }));

    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let alert_setup = match resolve_alert_rule(
        args.alert_component.as_deref(),
        args.alert_above,
        args.alert_for.as_deref(),
        args.alert_run.as_deref(),
    ) {
        Ok(setup) => setup,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let (alert_evaluator, alert_hook) = match alert_setup {
        Some((rule, hook)) => (Some(AlertEvaluator::new(rule)), hook),
        None => (None, None),
    };

    let mut sampler = build_default_sampler();
    if sampler.is_empty() {
        eprintln!("no power sensors available on this machine - nothing to show");
        std::process::exit(1);
    }

    let db_path = history_db_path();
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let storage = Storage::open(&db_path).ok();

    let mut terminal = setup_terminal()?;
    let result = run(
        &mut terminal,
        &mut sampler,
        storage,
        alert_evaluator,
        alert_hook,
    );
    restore_terminal()?;
    result
}

use state::{LoggingState, RunState, SuggestionsState};

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    sampler: &mut Sampler,
    storage: Option<Storage>,
    mut alert_evaluator: Option<AlertEvaluator>,
    alert_hook: Option<String>,
) -> io::Result<()> {
    let mut history = History::new(HISTORY_CAPACITY);
    let mut run_state = RunState::default();
    let mut logging_state = LoggingState::default();
    let mut suggestions_state = SuggestionsState::default();

    let mut suggestion_engine = SuggestionEngine::new(vec![
        SuggestionRule {
            component_name: "total".to_string(),
            threshold_watts: 75.0,
            sustained_for: Duration::from_secs(3),  // shortened from 10s
            message: "Total power draw has been high for a while. Consider switching to power saver profile.".to_string(),
            severity: powerwatch_core::suggestions::Severity::Warning,
            action: Some(powerwatch_core::suggestions::ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
        },
        SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 30.0,
            sustained_for: Duration::from_secs(3),  // shortened from 15s
            message: "CPU power draw has been high. Consider reducing workload or switching to power saver.".to_string(),
            severity: powerwatch_core::suggestions::Severity::Warning,
            action: Some(powerwatch_core::suggestions::ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
        },
    ]);

    let mut suggestions: Vec<Suggestion> = Vec::new();
    let mut last_snapshot = sampler.sample_all();
    history.record(&last_snapshot);
    let mut alert_active = false;
    draw(
        terminal,
        &last_snapshot,
        &history,
        run_state,
        logging_state,
        alert_active,
        &suggestions_state,
        &suggestions,
    )?;
    let mut last_sample = Instant::now();

    loop {
        if !run_state.is_paused() && last_sample.elapsed() >= SAMPLE_INTERVAL {
            last_snapshot = sampler.sample_all();
            history.record(&last_snapshot);
            if logging_state.is_logging() {
                record_to_storage(&storage, &last_snapshot);
            }
            if let Some(evaluator) = alert_evaluator.as_mut() {
                if evaluator.evaluate_at(&last_snapshot, Instant::now()) {
                    if let Some(command) = &alert_hook {
                        let _ = powerwatch_core::hook::run_hook(command);
                    }
                }
                alert_active = evaluator.is_active();
            }
            let new_suggestions = suggestion_engine.evaluate_at(&last_snapshot, Instant::now());
            for s in new_suggestions {
                suggestions.push(s);
            }

            draw(
                terminal,
                &last_snapshot,
                &history,
                run_state,
                logging_state,
                alert_active,
                &suggestions_state,
                &suggestions,
            )?;
            last_sample = Instant::now();
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('p') => {
                        run_state = run_state.toggle();
                        draw(
                            terminal,
                            &last_snapshot,
                            &history,
                            run_state,
                            logging_state,
                            alert_active,
                            &suggestions_state,
                            &suggestions,
                        )?;
                    }
                    KeyCode::Char('l') if storage.is_some() => {
                        logging_state = logging_state.toggle();
                        draw(
                            terminal,
                            &last_snapshot,
                            &history,
                            run_state,
                            logging_state,
                            alert_active,
                            &suggestions_state,
                            &suggestions,
                        )?;
                    }
                    KeyCode::Char('s') => {
                        match suggestions_state {
                            SuggestionsState::Closed => {
                                suggestions_state = SuggestionsState::Open;
                            }
                            _ => {
                                suggestions_state = SuggestionsState::Closed;
                            }
                        }
                        draw(
                            terminal,
                            &last_snapshot,
                            &history,
                            run_state,
                            logging_state,
                            alert_active,
                            &suggestions_state,
                            &suggestions,
                        )?;
                    }
                    KeyCode::Enter
                        if suggestions_state == SuggestionsState::Open
                            && !suggestions.is_empty() =>
                    {
                        if suggestions.iter().any(|s| s.action.is_some()) {
                            suggestions_state = SuggestionsState::Confirming;
                        }
                        draw(
                            terminal,
                            &last_snapshot,
                            &history,
                            run_state,
                            logging_state,
                            alert_active,
                            &suggestions_state,
                            &suggestions,
                        )?;
                    }
                    KeyCode::Char('y') | KeyCode::Char('Y')
                        if suggestions_state == SuggestionsState::Confirming
                            && !suggestions.is_empty() =>
                    {
                        if let Some(pos) = suggestions.iter().position(|s| s.action.is_some()) {
                            let s = suggestions.remove(pos);
                            if let Some(action) = s.action {
                                apply_action(&action);
                            }
                        }
                        suggestions_state = SuggestionsState::Open;
                        draw(
                            terminal,
                            &last_snapshot,
                            &history,
                            run_state,
                            logging_state,
                            alert_active,
                            &suggestions_state,
                            &suggestions,
                        )?;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N')
                        if suggestions_state == SuggestionsState::Confirming =>
                    {
                        suggestions_state = SuggestionsState::Open;
                        draw(
                            terminal,
                            &last_snapshot,
                            &history,
                            run_state,
                            logging_state,
                            alert_active,
                            &suggestions_state,
                            &suggestions,
                        )?;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn apply_action(action: &powerwatch_core::suggestions::ActionDescriptor) {
    use powerwatch_core::actions::PowerProfileActionExecutor;
    if let Err(e) = PowerProfileActionExecutor::for_current_os().apply(&action.kind) {
        eprintln!("failed to apply action: {e:?}");
    }
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    snapshot: &powerwatch_core::sampler::Snapshot,
    history: &History,
    run_state: RunState,
    logging_state: LoggingState,
    alert_active: bool,
    suggestions_state: &SuggestionsState,
    suggestions: &[Suggestion],
) -> io::Result<()> {
    terminal
        .draw(|frame| {
            render::render(
                frame,
                snapshot,
                history,
                run_state.is_paused(),
                logging_state.is_logging(),
                alert_active,
                suggestions_state,
                suggestions,
            )
        })
        .map(|_| ())
}

fn record_to_storage(storage: &Option<Storage>, snapshot: &powerwatch_core::sampler::Snapshot) {
    let Some(storage) = storage else { return };

    for reading in snapshot.readings() {
        let _ = storage.insert_reading(reading);
    }
    if let Some(total) = snapshot.total() {
        let _ = storage.insert_reading(&total);
    }
}
