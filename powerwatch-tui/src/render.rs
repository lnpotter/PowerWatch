use crate::history::History;
use powerwatch_core::actions::Platform;
use powerwatch_core::model::Confidence;
use powerwatch_core::sampler::Snapshot;
use powerwatch_core::suggestions::Suggestion;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Sparkline, Table};
use ratatui::Frame;

fn confidence_color(confidence: Confidence) -> Color {
    match confidence {
        Confidence::Measured => Color::Green,
        Confidence::Estimated => Color::Yellow,
    }
}

fn confidence_label(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Measured => "measured",
        Confidence::Estimated => "estimated",
    }
}

fn component_label(component: &powerwatch_core::model::Component) -> String {
    use powerwatch_core::model::{Component, GpuVendor};
    match component {
        Component::Cpu => "cpu".to_string(),
        Component::Gpu(GpuVendor::Nvidia) => "gpu (nvidia)".to_string(),
        Component::Gpu(GpuVendor::Amd) => "gpu (amd)".to_string(),
        Component::Gpu(GpuVendor::Intel) => "gpu (intel)".to_string(),
        Component::Gpu(GpuVendor::Apple) => "gpu (apple)".to_string(),
        Component::Ram => "ram".to_string(),
        Component::Disk(name) => format!("disk ({name})"),
        Component::Total => "total".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    snapshot: &Snapshot,
    history: &History,
    paused: bool,
    logging: bool,
    alert_active: bool,
    suggestions_state: &crate::state::SuggestionsState,
    suggestions: &[Suggestion],
    notice: Option<&str>,
) {
    let area = frame.size();

    if suggestions_state.is_open() {
        render_suggestions_panel(frame, area, suggestions_state, suggestions, notice);
        return;
    }

    let total_height = 3;
    let alert_height = if alert_active { 1 } else { 0 };
    let footer_height = 1;
    let sensor_count = snapshot.results.len().max(1) as u16;
    let remaining = area
        .height
        .saturating_sub(total_height + alert_height + footer_height);
    let sensor_height = (remaining / sensor_count).max(3);

    let mut constraints = vec![Constraint::Length(total_height)];
    if alert_active {
        constraints.push(Constraint::Length(alert_height));
    }
    for _ in &snapshot.results {
        constraints.push(Constraint::Length(sensor_height));
    }
    constraints.push(Constraint::Min(footer_height));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_total(frame, chunks[0], snapshot, paused, logging);

    let mut next_chunk = 1;
    if alert_active {
        render_alert_banner(frame, chunks[next_chunk]);
        next_chunk += 1;
    }

    for (i, (name, result)) in snapshot.results.iter().enumerate() {
        render_sensor(frame, chunks[next_chunk + i], name, result, history);
    }

    render_footer(
        frame,
        chunks[chunks.len() - 1],
        paused,
        logging,
        suggestions_state,
    );
}

fn render_suggestions_panel(
    frame: &mut Frame,
    area: Rect,
    state: &crate::state::SuggestionsState,
    suggestions: &[Suggestion],
    notice: Option<&str>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Suggestions ")
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, area);

    let inner = area.inner(&ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    if suggestions.is_empty() {
        frame.render_widget(Paragraph::new("No suggestions"), inner);
    } else {
        let header = Row::new(vec![
            Span::raw("  Message"),
            Span::styled("Action", Style::default().fg(Color::Yellow)),
        ]);

        let selected = state.selected();
        let rows: Vec<Row> = suggestions
            .iter()
            .enumerate()
            .map(|(index, s)| {
                let action = match &s.action {
                    Some(a) => a.label.clone(),
                    None => "(info only)".to_string(),
                };
                let action_color = if s.action.is_some() {
                    Color::Yellow
                } else {
                    Color::DarkGray
                };
                let is_selected = selected == Some(index);
                let marker = if is_selected { "> " } else { "  " };
                let row = Row::new(vec![
                    Span::raw(format!("{marker}{}", s.message)),
                    Span::styled(action, Style::default().fg(action_color)),
                ]);
                if is_selected {
                    row.style(Style::default().add_modifier(Modifier::REVERSED))
                } else {
                    row
                }
            })
            .collect();

        let table = Table::new(
            rows,
            [Constraint::Percentage(60), Constraint::Percentage(40)],
        )
        .header(header)
        .block(Block::default().borders(Borders::NONE));

        frame.render_widget(table, inner);
    }

    let footer_color = if notice.is_some() {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let command = match state {
        crate::state::SuggestionsState::Confirming { selected, .. } => suggestions
            .get(*selected)
            .and_then(|suggestion| suggestion.action.as_ref())
            .map(|action| Platform::current().command_line(&action.kind)),
        _ => None,
    };
    let footer = Paragraph::new(panel_footer(
        state,
        suggestions.is_empty(),
        notice,
        command.as_deref(),
    ))
    .style(Style::default().fg(footer_color));
    let footer_area = Rect {
        x: area.x,
        y: area.y + area.height - 1,
        width: area.width,
        height: 1,
    };
    frame.render_widget(footer, footer_area);
}

fn panel_footer(
    state: &crate::state::SuggestionsState,
    is_empty: bool,
    notice: Option<&str>,
    command: Option<&str>,
) -> String {
    match (notice, state) {
        (Some(message), _) => format!("  {message}  "),
        (None, crate::state::SuggestionsState::Confirming { .. }) => match command {
            Some(command) => format!("  [y] apply  [n] cancel   runs: {command}  "),
            None => "  [y] apply  [n] cancel  ".to_string(),
        },
        (None, _) if is_empty => "  [s] close  ".to_string(),
        (None, _) => "  [up/down] select  [enter] apply  [d] dismiss  [s] close  ".to_string(),
    }
}

fn render_alert_banner(frame: &mut Frame, area: Rect) {
    let banner = Paragraph::new(Span::styled(
        "⚠ ALERT: threshold sustained",
        Style::default()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    frame.render_widget(banner, area);
}

fn render_total(frame: &mut Frame, area: Rect, snapshot: &Snapshot, paused: bool, logging: bool) {
    let mut spans = match snapshot.total() {
        Some(total) => vec![
            Span::raw("total: "),
            Span::styled(
                format!("{:.1} W", total.watts),
                Style::default().fg(confidence_color(total.confidence)),
            ),
            Span::raw(format!(" ({})", confidence_label(total.confidence))),
        ],
        None if snapshot.results.is_empty() => vec![Span::raw("total: no data")],
        None => vec![Span::styled(
            "total: no data (all sensors failed)",
            Style::default().fg(Color::Red),
        )],
    };

    if paused {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "PAUSED",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
    }

    if logging {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "● logging",
            Style::default()
                .fg(Color::Red)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
    }

    let block = Block::default().borders(Borders::ALL).title("PowerWatch");
    frame.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn render_sensor(
    frame: &mut Frame,
    area: Rect,
    name: &str,
    result: &Result<powerwatch_core::model::SensorReading, powerwatch_core::model::SensorError>,
    history: &History,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    let range_text = history
        .window(name)
        .filter(|w| w.len() >= 2)
        .and_then(|w| Some((w.min()?, w.max()?)))
        .map(|(min, max)| format!("  range: {min:.1}-{max:.1} W"));

    let label_line = match result {
        Ok(reading) => {
            let mut spans = vec![
                Span::raw(format!("{}: ", component_label(&reading.component))),
                Span::styled(
                    format!("{:.1} W", reading.watts),
                    Style::default().fg(confidence_color(reading.confidence)),
                ),
                Span::raw(format!(" ({})", confidence_label(reading.confidence))),
            ];
            if let Some(range) = range_text {
                spans.push(Span::raw(range));
            }
            Line::from(spans)
        }
        Err(error) => Line::from(vec![
            Span::raw(format!("{name}: ")),
            Span::styled("unavailable", Style::default().fg(Color::Red)),
            Span::raw(format!(" ({error:?})")),
        ]),
    };
    frame.render_widget(Paragraph::new(label_line), chunks[0]);

    if let Some(window) = history.window(name) {
        if !window.is_empty() {
            let data: Vec<u64> = window.values().iter().map(|v| v.round() as u64).collect();
            let sparkline = Sparkline::default().data(&data);
            frame.render_widget(sparkline, chunks[1]);
        }
    }
}

fn render_footer(
    frame: &mut Frame,
    area: Rect,
    paused: bool,
    logging: bool,
    suggestions_state: &crate::state::SuggestionsState,
) {
    let pause_hint = if paused { "p: resume" } else { "p: pause" };
    let log_hint = if logging {
        "l: stop logging"
    } else {
        "l: start logging"
    };
    let suggestions_hint = if suggestions_state.is_open() {
        "s: close"
    } else {
        "s: suggestions"
    };
    let footer = Paragraph::new(format!(
        "q: quit  {pause_hint}  {log_hint}  {suggestions_hint}"
    ));
    frame.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SuggestionsState;
    use chrono::Utc;
    use powerwatch_core::model::{Component, SensorError, SensorReading};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn reading(component: Component, watts: f64, confidence: Confidence) -> SensorReading {
        SensorReading {
            component,
            watts,
            confidence,
            timestamp: Utc::now(),
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        let mut text = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                text.push_str(buffer.get(x, y).symbol());
            }
            text.push('\n');
        }
        text
    }

    fn suggestion(message: &str, with_action: bool) -> Suggestion {
        Suggestion {
            message: message.to_string(),
            component_name: "cpu".to_string(),
            severity: powerwatch_core::suggestions::Severity::Info,
            action: with_action.then(|| powerwatch_core::suggestions::ActionDescriptor {
                label: "Do it".to_string(),
                kind: powerwatch_core::suggestions::ActionKind::ShowTopProcesses,
            }),
        }
    }

    fn render_panel(state: SuggestionsState, suggestions: &[Suggestion]) -> String {
        render_panel_with_notice(state, suggestions, None)
    }

    fn render_panel_with_notice(
        state: SuggestionsState,
        suggestions: &[Suggestion],
        notice: Option<&str>,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: Vec::new(),
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &state,
                    suggestions,
                    notice,
                )
            })
            .unwrap();

        buffer_text(&terminal)
    }

    fn power_profile_suggestion(
        message: &str,
        profile: powerwatch_core::suggestions::PowerProfile,
    ) -> Suggestion {
        Suggestion {
            message: message.to_string(),
            component_name: "cpu".to_string(),
            severity: powerwatch_core::suggestions::Severity::Warning,
            action: Some(powerwatch_core::suggestions::ActionDescriptor {
                label: "Switch profile".to_string(),
                kind: powerwatch_core::suggestions::ActionKind::SetPowerProfile(profile),
            }),
        }
    }

    fn command_for(profile: powerwatch_core::suggestions::PowerProfile) -> String {
        Platform::current().command_line(
            &powerwatch_core::suggestions::ActionKind::SetPowerProfile(profile),
        )
    }

    #[test]
    fn the_confirmation_shows_the_command_that_will_run() {
        use powerwatch_core::suggestions::PowerProfile;
        let suggestions = [power_profile_suggestion("high", PowerProfile::PowerSaver)];

        let text = render_panel(
            SuggestionsState::Confirming { id: 1, selected: 0 },
            &suggestions,
        );

        assert!(text.contains(&format!("runs: {}", command_for(PowerProfile::PowerSaver))));
        assert!(text.contains("[y] apply"));
    }

    #[test]
    fn the_confirmation_shows_the_command_of_the_selected_row() {
        use powerwatch_core::suggestions::PowerProfile;
        let suggestions = [
            power_profile_suggestion("first", PowerProfile::PowerSaver),
            power_profile_suggestion("second", PowerProfile::Performance),
        ];

        let text = render_panel(
            SuggestionsState::Confirming { id: 2, selected: 1 },
            &suggestions,
        );

        assert!(text.contains(&format!("runs: {}", command_for(PowerProfile::Performance))));
        assert!(!text.contains(&command_for(PowerProfile::PowerSaver)));
    }

    #[test]
    fn the_command_is_not_shown_outside_a_confirmation() {
        use powerwatch_core::suggestions::PowerProfile;
        let suggestions = [power_profile_suggestion("high", PowerProfile::PowerSaver)];

        let text = render_panel(SuggestionsState::Open { selected: 0 }, &suggestions);

        assert!(!text.contains("runs:"));
    }

    #[test]
    fn confirming_a_suggestion_without_an_action_shows_no_command() {
        let suggestions = [suggestion("info only", false)];

        let text = render_panel(
            SuggestionsState::Confirming { id: 1, selected: 0 },
            &suggestions,
        );

        assert!(!text.contains("runs:"));
    }

    #[test]
    fn the_suggestions_panel_marks_the_selected_row() {
        let suggestions = [
            suggestion("first one", true),
            suggestion("second one", true),
        ];

        let text = render_panel(SuggestionsState::Open { selected: 1 }, &suggestions);

        assert!(text.contains("> second one"));
        assert!(!text.contains("> first one"));
        assert!(text.contains("  first one"));
    }

    #[test]
    fn the_suggestions_panel_lists_the_keys_for_selecting_and_dismissing() {
        let suggestions = [suggestion("first one", true)];

        let text = render_panel(SuggestionsState::Open { selected: 0 }, &suggestions);

        assert!(text.contains("[d] dismiss"));
        assert!(text.contains("[enter] apply"));
    }

    #[test]
    fn the_confirmation_footer_only_offers_apply_and_cancel() {
        let suggestions = [suggestion("first one", true)];

        let text = render_panel(
            SuggestionsState::Confirming { id: 1, selected: 0 },
            &suggestions,
        );

        assert!(text.contains("[y] apply"));
        assert!(text.contains("[n] cancel"));
        assert!(!text.contains("[d] dismiss"));
    }

    #[test]
    fn the_panel_has_an_action_column_header() {
        let suggestions = [suggestion("first one", true)];

        let text = render_panel(SuggestionsState::Open { selected: 0 }, &suggestions);

        assert!(text.contains("Message"));
        assert!(text.contains("Action"));
        assert!(!text.contains(" | "));
    }

    #[test]
    fn a_notice_replaces_the_key_hints_in_the_footer() {
        let suggestions = [suggestion("first one", true)];

        let text = render_panel_with_notice(
            SuggestionsState::Open { selected: 0 },
            &suggestions,
            Some("Failed: powerprofilesctl is missing"),
        );

        assert!(text.contains("Failed: powerprofilesctl is missing"));
        assert!(!text.contains("[d] dismiss"));
    }

    #[test]
    fn the_footer_is_still_drawn_when_the_list_is_empty() {
        let text = render_panel(SuggestionsState::Open { selected: 0 }, &[]);

        assert!(text.contains("No suggestions"));
        assert!(text.contains("[s] close"));
    }

    #[test]
    fn the_suggestions_panel_says_so_when_there_is_nothing_to_show() {
        let text = render_panel(SuggestionsState::Open { selected: 0 }, &[]);

        assert!(text.contains("No suggestions"));
    }

    #[test]
    fn shows_the_total_watts_and_confidence() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Ok(reading(Component::Cpu, 12.0, Confidence::Measured)),
            )],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("total"));
        assert!(text.contains("12.0 W"));
    }

    #[test]
    fn shows_each_sensors_name_and_watts() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![
                (
                    "cpu".to_string(),
                    Ok(reading(Component::Cpu, 8.5, Confidence::Measured)),
                ),
                (
                    "ram".to_string(),
                    Ok(reading(Component::Ram, 4.0, Confidence::Estimated)),
                ),
            ],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("cpu"));
        assert!(text.contains("8.5 W"));
        assert!(text.contains("ram"));
        assert!(text.contains("4.0 W"));
    }

    #[test]
    fn shows_unavailable_instead_of_a_wattage_for_a_failed_sensor() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "gpu".to_string(),
                Err(SensorError::Unavailable("no driver".to_string())),
            )],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("gpu"));
        assert!(text.contains("unavailable"));
    }

    #[test]
    fn shows_the_quit_hint_in_the_footer() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("quit"));
    }

    #[test]
    fn shows_a_paused_indicator_when_paused() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    true,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.to_lowercase().contains("paused"));
    }

    #[test]
    fn does_not_show_paused_when_running() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(!text.to_lowercase().contains("paused"));
    }

    #[test]
    fn shows_a_logging_indicator_when_logging() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    true,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("● logging"));
    }

    #[test]
    fn does_not_show_logging_indicator_when_idle() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(!text.contains("● logging"));
    }

    #[test]
    fn shows_an_alert_banner_when_active() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    true,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("ALERT"));
    }

    #[test]
    fn does_not_show_the_alert_banner_when_inactive() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(!text.contains("ALERT"));
    }

    #[test]
    fn does_not_panic_when_nothing_reported_successfully() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Err(SensorError::Unavailable("no RAPL".to_string())),
            )],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("no data"));
    }

    #[test]
    fn shows_the_min_max_range_from_recent_history() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Ok(reading(Component::Cpu, 10.0, Confidence::Measured)),
            )],
        };
        let mut history = History::new(10);
        history.record(&Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Ok(reading(Component::Cpu, 5.0, Confidence::Measured)),
            )],
        });
        history.record(&snapshot);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("5.0"));
        assert!(text.contains("10.0"));
    }

    #[test]
    fn explains_that_every_sensor_failed_instead_of_just_saying_no_data() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Err(SensorError::Unavailable("no RAPL".to_string())),
            )],
        };
        let history = History::new(10);

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    &history,
                    false,
                    false,
                    false,
                    &SuggestionsState::Closed,
                    &[],
                    None,
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("all sensors failed"));
    }
}
