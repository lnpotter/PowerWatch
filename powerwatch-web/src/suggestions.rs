use powerwatch_core::actions::{ActionError, Platform, PowerProfileActionExecutor};
use powerwatch_core::profile::{detect_current_profile, is_redundant};
use powerwatch_core::sampler::Snapshot;
use powerwatch_core::suggestion_board::SuggestionBoard;
use powerwatch_core::suggestions::{
    ActionDescriptor, ActionKind, PowerProfile, Severity, Suggestion, SuggestionRule,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN_TTL: Duration = Duration::from_secs(60);

struct IssuedToken {
    suggestion_id: u64,
    issued: Instant,
}

#[derive(Clone)]
pub struct SuggestionsState {
    board: Arc<Mutex<SuggestionBoard>>,
    tokens: Arc<Mutex<HashMap<String, IssuedToken>>>,
}

impl SuggestionsState {
    pub fn new() -> Self {
        Self::with_rules(default_rules())
    }

    pub(crate) fn with_rules(rules: Vec<SuggestionRule>) -> Self {
        Self {
            board: Arc::new(Mutex::new(SuggestionBoard::new(rules))),
            tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn evaluate(&self, snapshot: &Snapshot) {
        self.evaluate_at(snapshot, Instant::now(), detect_current_profile);
    }

    pub(crate) fn evaluate_at(
        &self,
        snapshot: &Snapshot,
        now: Instant,
        current_profile: impl Fn() -> Option<PowerProfile>,
    ) {
        self.board
            .lock()
            .unwrap()
            .update_at_filtered(snapshot, now, |suggestion| {
                !is_redundant(suggestion, &current_profile)
            });
    }

    pub fn list_with_tokens(&self) -> Vec<Proposal> {
        let board = self.board.lock().unwrap();
        let mut tokens = self.tokens.lock().unwrap();
        let now = Instant::now();
        tokens.retain(|_, t| {
            now.duration_since(t.issued) < TOKEN_TTL && board.get(t.suggestion_id).is_some()
        });

        board
            .active()
            .iter()
            .map(|active| {
                let token = active.suggestion.action.as_ref().map(|_| {
                    let existing = tokens
                        .iter()
                        .find(|(_, t)| t.suggestion_id == active.id)
                        .map(|(token, _)| token.clone());
                    existing.unwrap_or_else(|| {
                        let token = new_token();
                        tokens.insert(
                            token.clone(),
                            IssuedToken {
                                suggestion_id: active.id,
                                issued: now,
                            },
                        );
                        token
                    })
                });
                let command = active
                    .suggestion
                    .action
                    .as_ref()
                    .map(|action| Platform::current().command_line(&action.kind));
                Proposal {
                    id: active.id,
                    suggestion: active.suggestion.clone(),
                    token,
                    command,
                }
            })
            .collect()
    }

    pub fn apply(&self, token: &str) -> Result<bool, String> {
        self.apply_with(token, apply_action)
    }

    fn apply_with(
        &self,
        token: &str,
        run: impl FnOnce(&ActionDescriptor) -> Result<(), String>,
    ) -> Result<bool, String> {
        let issued = self
            .tokens
            .lock()
            .unwrap()
            .remove(token)
            .ok_or_else(|| "invalid or expired token".to_string())?;
        if issued.issued.elapsed() > TOKEN_TTL {
            return Err("token expired".to_string());
        }

        let action = self
            .board
            .lock()
            .unwrap()
            .get(issued.suggestion_id)
            .and_then(|active| active.suggestion.action.clone())
            .ok_or_else(|| "suggestion is no longer active".to_string())?;

        run(&action)?;

        self.board.lock().unwrap().dismiss(issued.suggestion_id);
        Ok(true)
    }

    pub fn dismiss(&self, id: u64) -> bool {
        self.board.lock().unwrap().dismiss(id)
    }
}

fn default_rules() -> Vec<SuggestionRule> {
    let power_saver = || {
        Some(ActionDescriptor {
            label: "Switch to power saver".to_string(),
            kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
        })
    };

    vec![
        SuggestionRule {
            component_name: "total".to_string(),
            threshold_watts: 15.0,
            sustained_for: Duration::from_secs(3),
            message: "Total power draw has been high. Consider switching to power saver profile."
                .to_string(),
            severity: Severity::Warning,
            action: power_saver(),
        },
        SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 8.0,
            sustained_for: Duration::from_secs(3),
            message:
                "CPU power draw has been high. Consider reducing workload or switching to power saver."
                    .to_string(),
            severity: Severity::Warning,
            action: power_saver(),
        },
        SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 8.0,
            sustained_for: Duration::from_secs(10),
            message: "CPU usage has been high for a while. Review top processes to identify what's consuming resources."
                .to_string(),
            severity: Severity::Info,
            action: Some(ActionDescriptor {
                label: "Show top processes".to_string(),
                kind: ActionKind::ShowTopProcesses,
            }),
        },
    ]
}

fn new_token() -> String {
    rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect()
}

#[derive(Serialize)]
pub struct Proposal {
    pub id: u64,
    pub suggestion: Suggestion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Deserialize)]
pub struct ApplyRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct DismissRequest {
    pub id: u64,
}

#[derive(Serialize)]
pub struct ApplyResponse {
    pub success: bool,
    pub message: String,
}

fn apply_action(action: &ActionDescriptor) -> Result<(), String> {
    PowerProfileActionExecutor::for_current_os()
        .apply(&action.kind)
        .map_err(|e| match e {
            ActionError::Unsupported(msg) | ActionError::ExecutionFailed(msg) => msg,
        })
}

#[cfg(test)]
pub(crate) fn state_with_one_active_suggestion() -> SuggestionsState {
    let state = SuggestionsState::with_rules(vec![SuggestionRule {
        component_name: "cpu".to_string(),
        threshold_watts: 1.0,
        sustained_for: Duration::ZERO,
        message: "high".to_string(),
        severity: Severity::Info,
        action: Some(ActionDescriptor {
            label: "Show top processes".to_string(),
            kind: ActionKind::ShowTopProcesses,
        }),
    }]);
    let snapshot = Snapshot {
        timestamp: chrono::Utc::now(),
        results: vec![(
            "cpu".to_string(),
            Ok(powerwatch_core::model::SensorReading {
                component: powerwatch_core::model::Component::Cpu,
                watts: 40.0,
                confidence: powerwatch_core::model::Confidence::Measured,
                timestamp: chrono::Utc::now(),
            }),
        )],
    };
    state.evaluate_at(&snapshot, Instant::now(), || None);
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use powerwatch_core::model::{Component, Confidence, SensorReading};

    fn snapshot_with(component: &str, watts: f64) -> Snapshot {
        Snapshot {
            timestamp: Utc::now(),
            results: vec![(
                component.to_string(),
                Ok(SensorReading {
                    component: Component::Cpu,
                    watts,
                    confidence: Confidence::Measured,
                    timestamp: Utc::now(),
                }),
            )],
        }
    }

    fn rule(component: &str, threshold: f64, label: &str) -> SuggestionRule {
        SuggestionRule {
            component_name: component.to_string(),
            threshold_watts: threshold,
            sustained_for: Duration::ZERO,
            message: label.to_string(),
            severity: Severity::Info,
            action: Some(ActionDescriptor {
                label: label.to_string(),
                kind: ActionKind::ShowTopProcesses,
            }),
        }
    }

    fn clock(seconds: u64) -> Instant {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *START.get_or_init(Instant::now) + Duration::from_secs(seconds)
    }

    fn tick_at(state: &SuggestionsState, watts: f64, seconds: u64) {
        state.evaluate_at(&snapshot_with("cpu", watts), clock(seconds), || None);
    }

    fn tick(state: &SuggestionsState, watts: f64) {
        tick_at(state, watts, 0);
    }

    fn messages(state: &SuggestionsState) -> Vec<String> {
        state
            .list_with_tokens()
            .into_iter()
            .map(|p| p.suggestion.message)
            .collect()
    }

    fn token_for(state: &SuggestionsState, message: &str) -> String {
        state
            .list_with_tokens()
            .into_iter()
            .find(|p| p.suggestion.message == message)
            .and_then(|p| p.token)
            .unwrap()
    }

    #[test]
    fn the_default_rules_match_the_documented_thresholds() {
        let rules: Vec<_> = default_rules()
            .into_iter()
            .map(|r| (r.component_name, r.threshold_watts, r.sustained_for))
            .collect();

        assert_eq!(
            rules,
            vec![
                ("total".to_string(), 15.0, Duration::from_secs(3)),
                ("cpu".to_string(), 8.0, Duration::from_secs(3)),
                ("cpu".to_string(), 8.0, Duration::from_secs(10)),
            ]
        );
    }

    #[test]
    fn a_new_state_has_no_suggestions() {
        assert!(SuggestionsState::new().list_with_tokens().is_empty());
    }

    #[test]
    fn a_token_applies_only_its_own_suggestion() {
        let state = SuggestionsState::with_rules(vec![
            rule("cpu", 10.0, "cpu"),
            rule("total", 10.0, "total"),
        ]);
        tick(&state, 40.0);
        let token = token_for(&state, "total");
        let mut ran = Vec::new();

        state
            .apply_with(&token, |action| {
                ran.push(action.label.clone());
                Ok(())
            })
            .unwrap();

        assert_eq!(ran, vec!["total"]);
        assert_eq!(messages(&state), vec!["cpu"]);
    }

    #[test]
    fn a_failed_action_is_reported_and_the_suggestion_stays() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);
        let token = token_for(&state, "cpu");

        let result = state.apply_with(&token, |_| Err("powerprofilesctl not found".to_string()));

        assert_eq!(result, Err("powerprofilesctl not found".to_string()));
        assert_eq!(messages(&state), vec!["cpu"]);
    }

    #[test]
    fn a_token_works_only_once() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);
        let token = token_for(&state, "cpu");

        assert!(state.apply_with(&token, |_| Ok(())).is_ok());
        assert!(state.apply_with(&token, |_| Ok(())).is_err());
    }

    #[test]
    fn an_unknown_token_runs_nothing() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);
        let mut ran = false;

        let result = state.apply_with("made-up", |_| {
            ran = true;
            Ok(())
        });

        assert!(result.is_err());
        assert!(!ran);
    }

    #[test]
    fn a_token_for_a_suggestion_whose_load_dropped_runs_nothing() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick_at(&state, 40.0, 0);
        let token = token_for(&state, "cpu");
        tick_at(&state, 1.0, 1);
        tick_at(&state, 1.0, 10);
        let mut ran = false;

        let result = state.apply_with(&token, |_| {
            ran = true;
            Ok(())
        });

        assert!(result.is_err());
        assert!(!ran);
    }

    #[test]
    fn a_token_for_a_dismissed_suggestion_runs_nothing() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);
        let proposal = state.list_with_tokens().remove(0);
        let mut ran = false;

        assert!(state.dismiss(proposal.id));
        let result = state.apply_with(&proposal.token.unwrap(), |_| {
            ran = true;
            Ok(())
        });

        assert!(result.is_err());
        assert!(!ran);
    }

    #[test]
    fn listing_again_reuses_the_token_instead_of_issuing_new_ones() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);

        let first = token_for(&state, "cpu");
        let second = token_for(&state, "cpu");

        assert_eq!(first, second);
        assert_eq!(state.tokens.lock().unwrap().len(), 1);
    }

    #[test]
    fn tokens_of_suggestions_that_went_away_are_dropped() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick_at(&state, 40.0, 0);
        token_for(&state, "cpu");

        tick_at(&state, 1.0, 1);
        tick_at(&state, 1.0, 10);
        state.list_with_tokens();

        assert!(state.tokens.lock().unwrap().is_empty());
    }

    #[test]
    fn a_suggestion_goes_away_once_the_load_has_stayed_down() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);

        tick_at(&state, 40.0, 0);
        assert_eq!(messages(&state), vec!["cpu"]);

        tick_at(&state, 1.0, 1);
        assert_eq!(messages(&state), vec!["cpu"]);

        tick_at(&state, 1.0, 10);
        assert!(messages(&state).is_empty());
    }

    #[test]
    fn a_brief_dip_keeps_the_suggestion_and_its_token() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick_at(&state, 40.0, 0);
        let before = token_for(&state, "cpu");

        tick_at(&state, 1.0, 1);
        tick_at(&state, 40.0, 2);

        assert_eq!(messages(&state), vec!["cpu"]);
        assert_eq!(token_for(&state, "cpu"), before);
    }

    #[test]
    fn a_dismissed_suggestion_stays_gone_until_the_next_excursion() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick_at(&state, 40.0, 0);
        let id = state.list_with_tokens()[0].id;

        assert!(state.dismiss(id));
        tick_at(&state, 40.0, 1);
        assert!(messages(&state).is_empty());

        tick_at(&state, 1.0, 2);
        tick_at(&state, 1.0, 10);
        tick_at(&state, 40.0, 11);
        let again = state.list_with_tokens();
        assert_eq!(again.len(), 1);
        assert_ne!(again[0].id, id);
    }

    #[test]
    fn dismissing_an_unknown_id_returns_false() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);

        assert!(!state.dismiss(99));
        assert_eq!(messages(&state), vec!["cpu"]);
    }

    #[test]
    fn suggestions_for_the_same_component_do_not_replace_each_other() {
        let state = SuggestionsState::with_rules(vec![
            rule("cpu", 10.0, "mild"),
            rule("cpu", 30.0, "severe"),
        ]);

        tick(&state, 40.0);

        assert_eq!(messages(&state), vec!["mild", "severe"]);
    }

    #[test]
    fn each_suggestion_is_listed_with_its_own_id() {
        let state = SuggestionsState::with_rules(vec![
            rule("cpu", 10.0, "mild"),
            rule("cpu", 30.0, "severe"),
        ]);
        tick(&state, 40.0);

        let ids: Vec<u64> = state.list_with_tokens().iter().map(|p| p.id).collect();

        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn a_power_profile_suggestion_is_skipped_when_already_in_that_profile() {
        let power_saver_rule = || SuggestionRule {
            action: Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
            ..rule("cpu", 10.0, "saver")
        };
        let snapshot = snapshot_with("cpu", 40.0);

        let already = SuggestionsState::with_rules(vec![power_saver_rule()]);
        already.evaluate_at(&snapshot, Instant::now(), || Some(PowerProfile::PowerSaver));
        assert!(already.list_with_tokens().is_empty());

        let balanced = SuggestionsState::with_rules(vec![power_saver_rule()]);
        balanced.evaluate_at(&snapshot, Instant::now(), || Some(PowerProfile::Balanced));
        assert_eq!(balanced.list_with_tokens().len(), 1);

        let unknown = SuggestionsState::with_rules(vec![power_saver_rule()]);
        unknown.evaluate_at(&snapshot, Instant::now(), || None);
        assert_eq!(unknown.list_with_tokens().len(), 1);
    }

    #[test]
    fn the_current_profile_is_not_looked_up_for_suggestions_without_a_profile_action() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "info")]);

        state.evaluate_at(&snapshot_with("cpu", 40.0), Instant::now(), || {
            panic!("the profile must not be detected for this suggestion")
        });

        assert_eq!(messages(&state), vec!["info"]);
    }

    #[test]
    fn a_proposal_carries_the_command_that_will_run() {
        let power_saver = SuggestionRule {
            action: Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            }),
            ..rule("cpu", 10.0, "saver")
        };
        let state = SuggestionsState::with_rules(vec![power_saver]);
        tick(&state, 40.0);

        let proposal = state.list_with_tokens().remove(0);

        let expected = Platform::current()
            .command_line(&ActionKind::SetPowerProfile(PowerProfile::PowerSaver));
        assert_eq!(proposal.command.as_deref(), Some(expected.as_str()));
        assert!(!expected.is_empty());
    }

    #[test]
    fn the_command_is_part_of_the_json_the_dashboard_receives() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);

        let json = serde_json::to_value(state.list_with_tokens()).unwrap();

        assert!(json[0]["command"].as_str().is_some_and(|c| !c.is_empty()));
    }

    #[test]
    fn a_suggestion_without_an_action_has_no_command() {
        let info_only = SuggestionRule {
            action: None,
            ..rule("cpu", 10.0, "info")
        };
        let state = SuggestionsState::with_rules(vec![info_only]);
        tick(&state, 40.0);

        let json = serde_json::to_value(state.list_with_tokens()).unwrap();

        assert!(json[0].get("command").is_none());
    }
}
