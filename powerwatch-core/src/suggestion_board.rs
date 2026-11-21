use crate::sampler::Snapshot;
use crate::suggestions::{Suggestion, SuggestionEngine, SuggestionRule};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSuggestion {
    pub id: u64,
    pub suggestion: Suggestion,
    rule: usize,
}

pub struct SuggestionBoard {
    engine: SuggestionEngine,
    active: Vec<ActiveSuggestion>,
    next_id: u64,
}

impl SuggestionBoard {
    pub fn new(rules: Vec<SuggestionRule>) -> Self {
        Self {
            engine: SuggestionEngine::new(rules),
            active: Vec::new(),
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
        self.active.retain(|a| engine.is_rule_active(a.rule));

        for (rule, suggestion) in fired {
            if !admit(&suggestion) {
                continue;
            }
            self.active.push(ActiveSuggestion {
                id: self.next_id,
                suggestion,
                rule,
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
        let before = self.active.len();
        self.active.retain(|a| a.id != id);
        self.active.len() != before
    }
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

    fn messages(board: &SuggestionBoard) -> Vec<&str> {
        board
            .active()
            .iter()
            .map(|a| a.suggestion.message.as_str())
            .collect()
    }

    #[test]
    fn a_fired_rule_becomes_an_active_suggestion_with_an_id() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);

        board.update_at(&snapshot(40.0), Instant::now());

        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 1);
    }

    #[test]
    fn nothing_is_active_while_below_the_threshold() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);

        board.update_at(&snapshot(5.0), Instant::now());

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_suggestion_goes_away_when_the_load_drops_back() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(5.0), now + Duration::from_secs(1));

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_suggestion_stays_while_the_load_stays_high() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        board.update_at(&snapshot(35.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(30.0), now + Duration::from_secs(2));

        assert_eq!(messages(&board), vec!["high"]);
        assert_eq!(board.active()[0].id, 1);
    }

    #[test]
    fn a_dismissed_suggestion_stays_gone_for_the_rest_of_the_excursion() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        assert!(board.dismiss(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(1));
        board.update_at(&snapshot(40.0), now + Duration::from_secs(2));

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_dismissed_suggestion_comes_back_with_a_new_id_on_the_next_excursion() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
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
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        board.update_at(&snapshot(40.0), Instant::now());

        assert!(!board.dismiss(99));
        assert_eq!(messages(&board), vec!["high"]);
    }

    #[test]
    fn rules_on_the_same_component_do_not_replace_each_other() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "mild"), rule(30.0, "severe")]);
        let now = Instant::now();

        board.update_at(&snapshot(40.0), now);
        assert_eq!(messages(&board), vec!["mild", "severe"]);

        board.update_at(&snapshot(20.0), now + Duration::from_secs(1));
        assert_eq!(messages(&board), vec!["mild"]);
    }

    #[test]
    fn dismissing_one_suggestion_leaves_the_others() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "mild"), rule(30.0, "severe")]);
        board.update_at(&snapshot(40.0), Instant::now());

        let mild = board.active()[0].id;
        board.dismiss(mild);

        assert_eq!(messages(&board), vec!["severe"]);
    }

    #[test]
    fn get_finds_an_active_suggestion_by_id() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        board.update_at(&snapshot(40.0), Instant::now());

        assert_eq!(board.get(1).unwrap().suggestion.message, "high");
        assert!(board.get(2).is_none());
    }

    #[test]
    fn a_rejected_suggestion_is_not_shown_nor_retried_during_the_excursion() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        let now = Instant::now();

        board.update_at_filtered(&snapshot(40.0), now, |_| false);
        board.update_at_filtered(&snapshot(40.0), now + Duration::from_secs(1), |_| true);

        assert!(board.active().is_empty());
    }

    #[test]
    fn a_failing_sensor_clears_the_suggestion() {
        let mut board = SuggestionBoard::new(vec![rule(10.0, "high")]);
        let now = Instant::now();
        board.update_at(&snapshot(40.0), now);

        let no_reading = Snapshot {
            timestamp: Utc::now(),
            results: Vec::new(),
        };
        board.update_at(&no_reading, now + Duration::from_secs(1));

        assert!(board.active().is_empty());
    }
}
