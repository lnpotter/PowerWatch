use crate::alerts::{AlertEvaluator, AlertRule};
use crate::sampler::Snapshot;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PowerProfile {
    PowerSaver,
    Balanced,
    Performance,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActionKind {
    SetPowerProfile(PowerProfile),
    ShowTopProcesses,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActionDescriptor {
    pub label: String,
    pub kind: ActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Severity {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Suggestion {
    pub message: String,
    pub component_name: String,
    pub severity: Severity,
    pub action: Option<ActionDescriptor>,
}

pub struct SuggestionRule {
    pub component_name: String,
    pub threshold_watts: f64,
    pub sustained_for: Duration,
    pub message: String,
    pub severity: Severity,
    pub action: Option<ActionDescriptor>,
}

pub struct SuggestionEngine {
    rules: Vec<(SuggestionRule, AlertEvaluator)>,
}

impl SuggestionEngine {
    pub fn new(rules: Vec<SuggestionRule>) -> Self {
        let rules = rules
            .into_iter()
            .map(|rule| {
                let evaluator = AlertEvaluator::new(AlertRule {
                    component_name: rule.component_name.clone(),
                    threshold_watts: rule.threshold_watts,
                    sustained_for: rule.sustained_for,
                });
                (rule, evaluator)
            })
            .collect();

        Self { rules }
    }

    pub fn evaluate_at(&mut self, snapshot: &Snapshot, now: Instant) -> Vec<Suggestion> {
        self.evaluate_rules_at(snapshot, now)
            .into_iter()
            .map(|(_, suggestion)| suggestion)
            .collect()
    }

    pub fn evaluate_rules_at(
        &mut self,
        snapshot: &Snapshot,
        now: Instant,
    ) -> Vec<(usize, Suggestion)> {
        let mut fired = Vec::new();

        for (index, (rule, evaluator)) in self.rules.iter_mut().enumerate() {
            if evaluator.evaluate_at(snapshot, now) {
                fired.push((
                    index,
                    Suggestion {
                        message: rule.message.clone(),
                        component_name: rule.component_name.clone(),
                        severity: rule.severity,
                        action: rule.action.clone(),
                    },
                ));
            }
        }

        fired
    }

    pub fn is_rule_active(&self, index: usize) -> bool {
        self.rules
            .get(index)
            .is_some_and(|(_, evaluator)| evaluator.is_active())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Component, Confidence, SensorReading};
    use chrono::Utc;

    fn snapshot_with(watts: f64) -> Snapshot {
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

    fn cpu_rule_with_action() -> SuggestionRule {
        SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 50.0,
            sustained_for: Duration::from_secs(5),
            message: "CPU has been high for a while".to_string(),
            severity: Severity::Warning,
            action: Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
        }
    }

    #[test]
    fn no_suggestion_while_below_the_threshold() {
        let mut engine = SuggestionEngine::new(vec![cpu_rule_with_action()]);
        let t0 = Instant::now();

        let suggestions = engine.evaluate_at(&snapshot_with(10.0), t0);

        assert!(suggestions.is_empty());
    }

    #[test]
    fn fires_once_when_sustained_above_the_threshold() {
        let mut engine = SuggestionEngine::new(vec![cpu_rule_with_action()]);
        let t0 = Instant::now();

        let first = engine.evaluate_at(&snapshot_with(80.0), t0);
        let second = engine.evaluate_at(&snapshot_with(80.0), t0 + Duration::from_secs(5));
        let third = engine.evaluate_at(&snapshot_with(80.0), t0 + Duration::from_secs(6));

        assert!(first.is_empty());
        assert_eq!(second.len(), 1);
        assert!(third.is_empty()); // does not repeat while still sustained
    }

    #[test]
    fn the_fired_suggestion_carries_the_rules_message_and_action() {
        let mut engine = SuggestionEngine::new(vec![cpu_rule_with_action()]);
        let t0 = Instant::now();
        engine.evaluate_at(&snapshot_with(80.0), t0);

        let suggestions = engine.evaluate_at(&snapshot_with(80.0), t0 + Duration::from_secs(5));

        assert_eq!(suggestions[0].message, "CPU has been high for a while");
        assert_eq!(suggestions[0].component_name, "cpu");
        assert_eq!(suggestions[0].severity, Severity::Warning);
        assert_eq!(
            suggestions[0].action,
            Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            })
        );
    }

    #[test]
    fn a_rule_without_an_action_produces_an_informational_suggestion() {
        let rule = SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 50.0,
            sustained_for: Duration::from_secs(1),
            message: "disk has been busy for a while".to_string(),
            severity: Severity::Info,
            action: None,
        };
        let mut engine = SuggestionEngine::new(vec![rule]);
        let t0 = Instant::now();
        engine.evaluate_at(&snapshot_with(80.0), t0);

        let suggestions = engine.evaluate_at(&snapshot_with(80.0), t0 + Duration::from_secs(1));

        assert_eq!(suggestions[0].action, None);
    }

    #[test]
    fn multiple_rules_are_evaluated_independently() {
        let fast_rule = SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 50.0,
            sustained_for: Duration::from_secs(1),
            message: "fast".to_string(),
            severity: Severity::Info,
            action: None,
        };
        let slow_rule = SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 50.0,
            sustained_for: Duration::from_secs(10),
            message: "slow".to_string(),
            severity: Severity::Info,
            action: None,
        };
        let mut engine = SuggestionEngine::new(vec![fast_rule, slow_rule]);
        let t0 = Instant::now();
        engine.evaluate_at(&snapshot_with(80.0), t0);

        let suggestions = engine.evaluate_at(&snapshot_with(80.0), t0 + Duration::from_secs(1));

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].message, "fast");
    }
}
