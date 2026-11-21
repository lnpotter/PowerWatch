mod history;
mod render;
mod state;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use history::History;
use powerwatch_core::alerts::{resolve_alert_rule, AlertEvaluator};
use powerwatch_core::profile::{detect_current_profile, is_redundant};
use powerwatch_core::runtime::build_default_sampler;
use powerwatch_core::sampler::{Sampler, Snapshot};
use powerwatch_core::storage::Storage;
use powerwatch_core::suggestion_board::SuggestionBoard;
use powerwatch_core::suggestions::{ActionDescriptor, PowerProfile, Suggestion};
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
    #[arg(long)]
    config: Option<PathBuf>,
}

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const NOTICE_DURATION: Duration = Duration::from_secs(5);
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
    let loaded = match powerwatch_core::config::load(args.config.as_deref()) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if let Some(notice) = &loaded.notice {
        eprintln!("{notice}");
    }
    let config = loaded.config;
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

    let board = SuggestionBoard::with_clear_after(
        config.suggestions.power_saver_rules(),
        config.suggestions.clear_after(),
    );

    let mut terminal = setup_terminal()?;
    let result = run(
        &mut terminal,
        &mut sampler,
        storage,
        board,
        alert_evaluator,
        alert_hook,
    );
    restore_terminal()?;
    result
}

use state::{
    LoggingState, RunState, SuggestionItem, SuggestionsEffect, SuggestionsKey, SuggestionsState,
};

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    sampler: &mut Sampler,
    storage: Option<Storage>,
    mut board: SuggestionBoard,
    mut alert_evaluator: Option<AlertEvaluator>,
    alert_hook: Option<String>,
) -> io::Result<()> {
    let mut history = History::new(HISTORY_CAPACITY);
    let mut run_state = RunState::default();
    let mut logging_state = LoggingState::default();
    let mut suggestions_state = SuggestionsState::default();

    let mut suggestions: Vec<Suggestion> = Vec::new();
    let mut notice: Option<(String, Instant)> = None;
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
        notice_text(&notice),
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
            if notice
                .as_ref()
                .is_some_and(|(_, shown)| shown.elapsed() >= NOTICE_DURATION)
            {
                notice = None;
            }
            update_board(
                &mut board,
                &last_snapshot,
                Instant::now(),
                detect_current_profile,
            );
            suggestions = visible_suggestions(&board);
            suggestions_state = suggestions_state.synced(&suggestion_items(&board));

            draw(
                terminal,
                &last_snapshot,
                &history,
                run_state,
                logging_state,
                alert_active,
                &suggestions_state,
                &suggestions,
                notice_text(&notice),
            )?;
            last_sample = Instant::now();
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let suggestions_key = suggestions_key_for(key.code, suggestions_state);
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
                            notice_text(&notice),
                        )?;
                    }
                    KeyCode::Char('l') if storage.is_some() => {
                        logging_state = logging_state.toggle();
                        if logging_state.is_logging() {
                            if let Some(storage) = &storage {
                                let _ = storage.prune_expired();
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
                            notice_text(&notice),
                        )?;
                    }
                    _ => {
                        if let Some(suggestions_key) = suggestions_key {
                            let items = suggestion_items(&board);
                            let (next, effect) = suggestions_state.handle(suggestions_key, &items);
                            suggestions_state = next;
                            notice = None;
                            if let Some(message) = run_effect(effect, &mut board, apply_action) {
                                notice = Some((message, Instant::now()));
                            }
                            suggestions = visible_suggestions(&board);
                            suggestions_state = suggestions_state.synced(&suggestion_items(&board));
                            draw(
                                terminal,
                                &last_snapshot,
                                &history,
                                run_state,
                                logging_state,
                                alert_active,
                                &suggestions_state,
                                &suggestions,
                                notice_text(&notice),
                            )?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn update_board(
    board: &mut SuggestionBoard,
    snapshot: &Snapshot,
    now: Instant,
    current_profile: impl Fn() -> Option<PowerProfile>,
) {
    board.update_at_filtered(snapshot, now, |suggestion| {
        !is_redundant(suggestion, &current_profile)
    });
}

fn visible_suggestions(board: &SuggestionBoard) -> Vec<Suggestion> {
    board
        .active()
        .iter()
        .map(|a| a.suggestion.clone())
        .collect()
}

fn suggestion_items(board: &SuggestionBoard) -> Vec<SuggestionItem> {
    board
        .active()
        .iter()
        .map(|a| SuggestionItem {
            id: a.id,
            has_action: a.suggestion.action.is_some(),
        })
        .collect()
}

fn suggestions_key_for(code: KeyCode, state: SuggestionsState) -> Option<SuggestionsKey> {
    match code {
        KeyCode::Char('s') => Some(SuggestionsKey::Toggle),
        _ if !state.is_open() => None,
        KeyCode::Down | KeyCode::Char('j') => Some(SuggestionsKey::Next),
        KeyCode::Up | KeyCode::Char('k') => Some(SuggestionsKey::Previous),
        KeyCode::Enter => Some(SuggestionsKey::Confirm),
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(SuggestionsKey::Apply),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(SuggestionsKey::Cancel),
        KeyCode::Char('d') | KeyCode::Char('D') => Some(SuggestionsKey::Dismiss),
        _ => None,
    }
}

fn apply_action(action: &ActionDescriptor) -> Result<(), String> {
    use powerwatch_core::actions::{ActionError, PowerProfileActionExecutor};
    PowerProfileActionExecutor::for_current_os()
        .apply(&action.kind)
        .map_err(|e| match e {
            ActionError::Unsupported(message) | ActionError::ExecutionFailed(message) => message,
        })
}

fn run_effect(
    effect: SuggestionsEffect,
    board: &mut SuggestionBoard,
    apply: impl FnOnce(&ActionDescriptor) -> Result<(), String>,
) -> Option<String> {
    match effect {
        SuggestionsEffect::Nothing => None,
        SuggestionsEffect::Dismiss(id) => {
            board.dismiss(id);
            None
        }
        SuggestionsEffect::Apply(id) => {
            let action = board.get(id).and_then(|a| a.suggestion.action.clone())?;
            match apply(&action) {
                Ok(()) => {
                    board.dismiss(id);
                    Some(format!("Applied: {}", action.label))
                }
                Err(error) => Some(format!("Failed: {error}")),
            }
        }
    }
}

fn notice_text(notice: &Option<(String, Instant)>) -> Option<&str> {
    notice.as_ref().map(|(text, _)| text.as_str())
}

#[allow(clippy::too_many_arguments)]
fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    snapshot: &powerwatch_core::sampler::Snapshot,
    history: &History,
    run_state: RunState,
    logging_state: LoggingState,
    alert_active: bool,
    suggestions_state: &SuggestionsState,
    suggestions: &[Suggestion],
    notice: Option<&str>,
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
                notice,
            )
        })
        .map(|_| ())
}

fn record_to_storage(storage: &Option<Storage>, snapshot: &powerwatch_core::sampler::Snapshot) {
    let Some(storage) = storage else { return };

    let _ = storage.insert_snapshot(snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerwatch_core::suggestions::{ActionKind, SuggestionRule};

    fn open() -> SuggestionsState {
        SuggestionsState::Open { selected: 0 }
    }

    #[test]
    fn s_toggles_the_panel_whether_or_not_it_is_open() {
        assert_eq!(
            suggestions_key_for(KeyCode::Char('s'), SuggestionsState::Closed),
            Some(SuggestionsKey::Toggle)
        );
        assert_eq!(
            suggestions_key_for(KeyCode::Char('s'), open()),
            Some(SuggestionsKey::Toggle)
        );
    }

    #[test]
    fn panel_keys_do_nothing_while_the_panel_is_closed() {
        for code in [
            KeyCode::Char('d'),
            KeyCode::Char('y'),
            KeyCode::Char('n'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
        ] {
            assert_eq!(suggestions_key_for(code, SuggestionsState::Closed), None);
        }
    }

    #[test]
    fn open_panel_keys_map_to_their_actions() {
        let cases = [
            (KeyCode::Down, SuggestionsKey::Next),
            (KeyCode::Char('j'), SuggestionsKey::Next),
            (KeyCode::Up, SuggestionsKey::Previous),
            (KeyCode::Char('k'), SuggestionsKey::Previous),
            (KeyCode::Enter, SuggestionsKey::Confirm),
            (KeyCode::Char('y'), SuggestionsKey::Apply),
            (KeyCode::Char('Y'), SuggestionsKey::Apply),
            (KeyCode::Char('n'), SuggestionsKey::Cancel),
            (KeyCode::Char('N'), SuggestionsKey::Cancel),
            (KeyCode::Char('d'), SuggestionsKey::Dismiss),
            (KeyCode::Char('D'), SuggestionsKey::Dismiss),
        ];

        for (code, expected) in cases {
            assert_eq!(suggestions_key_for(code, open()), Some(expected));
        }
    }

    #[test]
    fn other_keys_are_left_to_the_rest_of_the_app() {
        for code in [KeyCode::Char('q'), KeyCode::Char('p'), KeyCode::Char('l')] {
            assert_eq!(suggestions_key_for(code, open()), None);
        }
    }

    fn power_saver_board() -> SuggestionBoard {
        use powerwatch_core::suggestions::Severity;

        SuggestionBoard::new(vec![SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 1.0,
            sustained_for: Duration::ZERO,
            message: "high".to_string(),
            severity: Severity::Info,
            action: Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
        }])
    }

    fn busy_cpu() -> Snapshot {
        use chrono::Utc;
        use powerwatch_core::model::{Component, Confidence, SensorReading};

        Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Ok(SensorReading {
                    component: Component::Cpu,
                    watts: 40.0,
                    confidence: Confidence::Measured,
                    timestamp: Utc::now(),
                }),
            )],
        }
    }

    #[test]
    fn power_saver_is_not_suggested_when_already_in_that_profile() {
        let mut board = power_saver_board();

        update_board(&mut board, &busy_cpu(), Instant::now(), || {
            Some(PowerProfile::PowerSaver)
        });

        assert!(board.active().is_empty());
    }

    #[test]
    fn power_saver_is_suggested_from_any_other_profile() {
        for profile in [
            Some(PowerProfile::Balanced),
            Some(PowerProfile::Performance),
            None,
        ] {
            let mut board = power_saver_board();

            update_board(&mut board, &busy_cpu(), Instant::now(), || profile);

            assert_eq!(board.active().len(), 1);
        }
    }

    fn board_with_one_suggestion() -> (SuggestionBoard, u64) {
        use chrono::Utc;
        use powerwatch_core::model::{Component, Confidence, SensorReading};
        use powerwatch_core::suggestions::Severity;

        let mut board = SuggestionBoard::new(vec![SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 1.0,
            sustained_for: Duration::ZERO,
            message: "high".to_string(),
            severity: Severity::Info,
            action: Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
        }]);
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Ok(SensorReading {
                    component: Component::Cpu,
                    watts: 40.0,
                    confidence: Confidence::Measured,
                    timestamp: Utc::now(),
                }),
            )],
        };
        board.update_at(&snapshot, Instant::now());
        let id = board.active()[0].id;
        (board, id)
    }

    #[test]
    fn a_failed_action_keeps_the_suggestion_and_says_why() {
        let (mut board, id) = board_with_one_suggestion();

        let notice = run_effect(SuggestionsEffect::Apply(id), &mut board, |_| {
            Err("powerprofilesctl is missing".to_string())
        });

        assert_eq!(
            notice.as_deref(),
            Some("Failed: powerprofilesctl is missing")
        );
        assert_eq!(board.active().len(), 1);
    }

    #[test]
    fn a_successful_action_removes_the_suggestion_and_says_so() {
        let (mut board, id) = board_with_one_suggestion();

        let notice = run_effect(SuggestionsEffect::Apply(id), &mut board, |_| Ok(()));

        assert_eq!(notice.as_deref(), Some("Applied: Switch to power saver"));
        assert!(board.active().is_empty());
    }

    #[test]
    fn dismissing_removes_the_suggestion_without_running_anything() {
        let (mut board, id) = board_with_one_suggestion();

        let notice = run_effect(SuggestionsEffect::Dismiss(id), &mut board, |_| {
            panic!("dismissing must not run an action")
        });

        assert_eq!(notice, None);
        assert!(board.active().is_empty());
    }

    #[test]
    fn applying_a_suggestion_that_is_gone_runs_nothing() {
        let (mut board, id) = board_with_one_suggestion();
        board.dismiss(id);

        let notice = run_effect(SuggestionsEffect::Apply(id), &mut board, |_| {
            panic!("a missing suggestion must not run an action")
        });

        assert_eq!(notice, None);
    }
}
