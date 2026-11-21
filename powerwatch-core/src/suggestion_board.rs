use crate::sampler::Snapshot;
use crate::suggestions::{Suggestion, SuggestionEngine, SuggestionRule};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const DEFAULT_CLEAR_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSuggestion {
    pub id: u64,
    pub suggestion: Suggestion,
    rule: usize,
    quiet_since: Option<Instant>,
}

pub struct SuggestionBoard {
    engine: SuggestionEngine,
    active: Vec<ActiveSuggestion>,
    dismissed: HashMap<usize, Option<Instant>>,
    clear_after: Duration,
    next_id: u64,
}

impl SuggestionBoard {
    pub fn new(rules: Vec<SuggestionRule>) -> Self {
        Self::with_clear_after(rules, DEFAULT_CLEAR_AFTER)
    }

    pub fn with_clear_after(rules: Vec<SuggestionRule>, clear_after: Duration) -> Self {
        Self {
            engine: SuggestionEngine::new(rules),
            active: Vec::new(),
            dismissed: HashMap::new(),
            clear_after,
            next_id: 1,
        }
    }

    pub fn update_at(&mut self, snapshot: &Snapshot, now: Instant) {
        self.update_at_filtered(snapshot, now, |_| true);
    }

    pub fn update_at_filtered(
        &mut self,
        snapshot: &Snapshot,
        now: Instant,
        mut admit: impl FnMut(&Suggestion) -> bool,
    ) {
        let fired = self.engine.evaluate_rules_at(snapshot, now);

        let engine = &self.engine;
        let clear_after = self.clear_after;

        for active in &mut self.active {
            track_quiet(
                &mut active.quiet_since,
                engine.is_rule_active(active.rule),
                now,
            );
        }
        self.active
            .retain(|active| !is_cleared(active.quiet_since, now, clear_after));

        for (rule, quiet_since) in self.dismissed.iter_mut() {
            track_quiet(quiet_since, engine.is_rule_active(*rule), now);
        }
        self.dismissed
            .retain(|_, quiet_since| !is_cleared(*quiet_since, now, clear_after));

        for (rule, suggestion) in fired {
            let already_shown = self.active.iter().any(|active| active.rule == rule);
            if already_shown || self.dismissed.contains_key(&rule) || !admit(&suggestion) {
                continue;
            }
            self.active.push(ActiveSuggestion {
                id: self.next_id,
                suggestion,
                rule,
                quiet_since: None,
            });
            self.next_id += 1;
        }
    }

    pub fn active(&self) -> &[ActiveSuggestion] {
        &self.active
    }

    pub fn get(&self, id: u64) -> Option<&ActiveSuggestion> {
        self.active.iter().find(|a| a.id == id)
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        let Some(position) = self.active.iter().position(|a| a.id == id) else {
            return false;
        };
        let removed = self.active.remove(position);
        self.dismissed.insert(removed.rule, None);
        true
    }
}

fn track_quiet(quiet_since: &mut Option<Instant>, rule_is_active: bool, now: Instant) {
    if rule_is_active {
        *quiet_since = None;
    } else if quiet_since.is_none() {
        *quiet_since = Some(now);
    }
}

fn is_cleared(quiet_since: Option<Instant>, now: Instant, clear_after: Duration) -> bool {
    quiet_since.is_some_and(|since| now.duration_since(since) >= clear_after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Component, Confidence, SensorReading};
    use crate::suggestions::Severity;
    use chrono::Utc;
    use std::time::Duration;

    fn snapshot(watts: f64) -> Snapshot {
        Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                "cpu".to_string(),
                Ok(SensorReading {
                    component: Component::Cpu,
                    watts,
                    confidence: Confidence::Measured,
                    timestamp: Utc::now(),
                }),
            )],
        }
    }

    fn rule(threshold: f64, message: &str) -> SuggestionRule {
        SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: threshold,
            sustained_for: Duration::ZERO,
            message: message.to_string(),
            severity: Severity::Info,
            action: None,
        }
    }

    fn immediate_board(rules: Vec<SuggestionRule>) -> SuggestionBoard {
        SuggestionBoard::with_clear_after(rules, Duration::ZERO)
    }

    fn patient_board(rules: Vec<SuggestionRule>) -> SuggestionBoard {
        SuggestionBoard::with_clear_after(rules, Duration::from_secs(5))
    }

    fn messages(board: &SuggestionBoard) -> Vec<&str> {
        board
            .active()
            .iter()
            .map(|a| a.suggestion.message.as_str())
            .collect()
    }

    #[test]
    fn a_fired_rule_becomes_an_active_suggestion_with_an_id() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);

        board.update_at(&snapshot(40.0), Instant::now());

        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 1);
    }

    #[test]
    fn nothing_is_active_while_below_the_threshold() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);

        board.update_at(&snapshot(5.0), Instant::now());

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_suggestion_goes_away_when_the_load_drops_back() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_suggestion_stays_while_the_load_stays_high() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(35.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(30.0), now + Duration::from_secs(2));

        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 1);
    }

    #[test]
    fn a_dismissed_suggestion_stays_gone_for_the_rest_of_the_excursion() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        assert!(board.dismiss(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(2));

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_dismissed_suggestion_comes_back_with_a_new_id_on_the_next_excursion() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.dismiss(1);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(2));

        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 2);
    }

    #[test]
    fn dismissing_an_unknown_id_changes_nothing() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        board.update_at(&snapshot(40.0), Instant::now());

        assert!(!board.dismiss(99));
        assert_eq!(messages(&board), vec!["high"]);
    }

    #[test]
    fn rules_on_the_same_component_do_not_replace_each_other() {
        let mut board = immediate_board(vec![rule(10.0, "mild"), rule(30.0, "severe")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        assert_eq!(messages(&board), vec!["mild", "severe"]);

        board.update_at(&snapshot(20.0), now + Duration::from_secs(1));
        assert_eq!(messages(&board), vec!["mild"]);
    }

    #[test]
    fn dismissing_one_suggestion_leaves_the_others() {
        let mut board = immediate_board(vec![rule(10.0, "mild"), rule(30.0, "severe")]);
        board.update_at(&snapshot(40.0), Instant::now());

        let mild = board.active()[0].id;
        board.dismiss(mild);

        assert_eq!(messages(&board), vec!["severe"]);
    }

    #[test]
    fn get_finds_an_active_suggestion_by_id() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        board.update_at(&snapshot(40.0), Instant::now());

        assert_eq!(board.get(1).unwrap().suggestion.message, "high");
        assert!(board.get(2).is_none());
    }

    #[test]
    fn a_rejected_suggestion_is_not_shown_nor_retried_during_the_excursion() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at_filtered(&snapshot(40.0), now, |_| false);
        board.update_at_filtered(&snapshot(40.0), now + Duration::from_secs(1), |_| true);

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_failing_sensor_clears_the_suggestion() {
        let mut board = immediate_board(vec![rule(10.0, "high")]);
        let now = Instant::now();
        board.update_at(&snapshot(40.0), now);

        let no_reading = Snapshot {
            timestamp: Utc::now(),
            results: Vec::new(),
        };
        board.update_at(&no_reading, now + Duration::from_secs(1));

        assert!(board.active().is_empty());
    }

    #[test]
    fn the_default_board_waits_a_few_seconds_before_clearing() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));

        assert_eq!(messages(&board), vec!["high"]);
    }

    #[test]
    fn a_short_dip_does_not_clear_the_suggestion_nor_duplicate_it() {
        let mut board = patient_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        assert_eq!(messages(&board), vec!["high"]);

        board.update_at(&snapshot(40.0), now + Duration::from_secs(2));
        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 1);
    }

    #[test]
    fn a_lasting_drop_clears_the_suggestion_after_the_grace_period() {
        let mut board = patient_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(5.0), now + Duration::from_secs(5));
        assert_eq!(messages(&board), vec!["high"]);

        board.update_at(&snapshot(5.0), now + Duration::from_secs(6));
        assert!(board.active().is_empty());
    }

    #[test]
    fn the_grace_period_starts_over_when_the_load_comes_back() {
        let mut board = patient_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(4));
        board.update_at(&snapshot(5.0), now + Duration::from_secs(6));
        board.update_at(&snapshot(5.0), now + Duration::from_secs(10));
        assert_eq!(messages(&board), vec!["high"]);

        board.update_at(&snapshot(5.0), now + Duration::from_secs(11));
        assert!(board.active().is_empty());
    }

    #[test]
    fn a_dismissed_suggestion_stays_gone_through_a_short_dip() {
        let mut board = patient_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.dismiss(1);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(2));

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_dismissed_suggestion_returns_after_a_lasting_drop() {
        let mut board = patient_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.dismiss(1);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(5.0), now + Duration::from_secs(7));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(8));

        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 2);
    }

    #[test]
    fn dismissing_during_the_grace_period_removes_it_right_away() {
        let mut board = patient_board(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));
        assert!(board.dismiss(1));

        assert!(board.active().is_empty());
    }

    fn ids_seen_over(board: &mut SuggestionBoard, readings: &[f64]) -> Vec<u64> {
        let start = Instant::now();
        let mut seen = Vec::new();
        for (second, watts) in readings.iter().enumerate() {
            board.update_at(
                &snapshot(*watts),
                start + Duration::from_secs(second as u64),
            );
            for active in board.active() {
                if !seen.contains(&active.id) {
                    seen.push(active.id);
                }
            }
        }
        seen
    }

    #[test]
    fn a_load_hovering_around_the_threshold_gives_one_stable_suggestion() {
        let hovering = [
            9.0, 9.1, 9.0, 9.2, 7.5, 9.2, 9.0, 9.1, 9.3, 7.9, 9.5, 9.1, 9.0, 9.2, 7.0, 9.3, 9.0,
            9.0, 9.1, 9.2,
        ];
        let sustained_rule = || SuggestionRule {
            sustained_for: Duration::from_secs(3),
            ..rule(8.0, "high")
        };

        let mut patient = patient_board(vec![sustained_rule()]);
        let mut immediate = immediate_board(vec![sustained_rule()]);

        assert_eq!(ids_seen_over(&mut patient, &hovering), vec![1]);
        assert!(ids_seen_over(&mut immediate, &hovering).len() > 1);
        assert_eq!(messages(&patient), vec!["high"]);
    }
}
