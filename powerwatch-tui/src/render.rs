use crate::history::History;
use powerwatch_core::model::Confidence;
use powerwatch_core::sampler::Snapshot;
use powerwatch_core::suggestions::Suggestion;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
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
) {
    let area = frame.size();

    if suggestions_state.is_open() {
        render_suggestions_panel(frame, area, suggestions_state, suggestions);
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
        let text = Paragraph::new("No suggestions");
        frame.render_widget(text, inner);
        return;
    }

    let header = Line::from(vec![
        Span::raw(" Message"),
        Span::raw(" | "),
        Span::styled(" Action", Style::default().fg(Color::Yellow)),
    ]);

    let rows: Vec<Row> = suggestions
        .iter()
        .map(|s| {
            let action = match &s.action {
                Some(a) => a.label.clone(),
                None => "(info only)".to_string(),
            };
            let action_color = if s.action.is_some() {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            Row::new(vec![
                Span::raw(s.message.clone()),
                Span::styled(action, Style::default().fg(action_color)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .header(Row::new(header))
    .block(Block::default().borders(Borders::NONE));

    frame.render_widget(table, inner);
    let footer_text = match state {
        crate::state::SuggestionsState::Confirming => "  [y] apply  [n] cancel  [any] cancel ",
        _ => "  [s] close  [enter] apply first action  ",
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    let footer_area = Rect {
        x: area.x,
        y: area.y + area.height - 1,
        width: area.width,
        height: 1,
    };
    frame.render_widget(footer, footer_area);
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
                )
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("all sensors failed"));
    }
}
