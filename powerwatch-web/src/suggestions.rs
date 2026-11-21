use powerwatch_core::actions::{ActionError, PowerProfileActionExecutor};
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
                Proposal {
                    id: active.id,
                    suggestion: active.suggestion.clone(),
                    token,
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

fn is_redundant(
    suggestion: &Suggestion,
    current_profile: &impl Fn() -> Option<PowerProfile>,
) -> bool {
    match suggestion.action.as_ref().map(|action| &action.kind) {
        Some(ActionKind::SetPowerProfile(target)) => current_profile() == Some(*target),
        _ => false,
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

fn detect_current_profile() -> Option<PowerProfile> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("powerprofilesctl").arg("get").output() {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    let text = text.trim();
                    if text.contains("power-save") {
                        return Some(PowerProfile::PowerSaver);
                    } else if text.contains("performance") {
                        return Some(PowerProfile::Performance);
                    } else {
                        return Some(PowerProfile::Balanced);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("pmset").args(&["get", "current"]).output() {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if text.contains("lowpowermode 1") {
                        return Some(PowerProfile::PowerSaver);
                    } else if text.contains("lowpowermode 0") {
                        return Some(PowerProfile::Balanced);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("powercfg").arg("/getactivescheme").output() {
            if output.status.success() {
                return parse_windows_active_scheme(&String::from_utf8_lossy(&output.stdout));
            }
        }
    }

    None
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_windows_active_scheme(text: &str) -> Option<PowerProfile> {
    let text = text.to_ascii_lowercase();
    if text.contains("a1841308-3541-4fab-bc81-f71556f20b4a") {
        Some(PowerProfile::PowerSaver)
    } else if text.contains("381b4222-f694-41f0-9685-ff5bb260df2e") {
        Some(PowerProfile::Balanced)
    } else if text.contains("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c")
        || text.contains("e9a42b02-d5df-448d-aa00-03f14749eb61")
    {
        Some(PowerProfile::Performance)
    } else {
        None
    }
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

    fn tick(state: &SuggestionsState, watts: f64) {
        state.evaluate_at(&snapshot_with("cpu", watts), Instant::now(), || None);
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
        tick(&state, 40.0);
        let token = token_for(&state, "cpu");
        tick(&state, 1.0);
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
        tick(&state, 40.0);
        token_for(&state, "cpu");

        tick(&state, 1.0);
        state.list_with_tokens();

        assert!(state.tokens.lock().unwrap().is_empty());
    }

    #[test]
    fn a_suggestion_goes_away_when_the_load_drops() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);

        tick(&state, 40.0);
        assert_eq!(messages(&state), vec!["cpu"]);

        tick(&state, 1.0);
        assert!(messages(&state).is_empty());
    }

    #[test]
    fn a_dismissed_suggestion_stays_gone_until_the_next_excursion() {
        let state = SuggestionsState::with_rules(vec![rule("cpu", 10.0, "cpu")]);
        tick(&state, 40.0);
        let id = state.list_with_tokens()[0].id;

        assert!(state.dismiss(id));
        tick(&state, 40.0);
        assert!(messages(&state).is_empty());

        tick(&state, 1.0);
        tick(&state, 40.0);
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
    fn windows_scheme_guids_map_to_profiles() {
        let line = |guid: &str, name: &str| format!("Power Scheme GUID: {guid}  ({name})\n");

        assert_eq!(
            parse_windows_active_scheme(&line(
                "a1841308-3541-4fab-bc81-f71556f20b4a",
                "Power saver"
            )),
            Some(PowerProfile::PowerSaver)
        );
        assert_eq!(
            parse_windows_active_scheme(&line("381b4222-f694-41f0-9685-ff5bb260df2e", "Balanced")),
            Some(PowerProfile::Balanced)
        );
        assert_eq!(
            parse_windows_active_scheme(&line(
                "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c",
                "High performance"
            )),
            Some(PowerProfile::Performance)
        );
        assert_eq!(
            parse_windows_active_scheme(&line(
                "e9a42b02-d5df-448d-aa00-03f14749eb61",
                "Ultimate Performance"
            )),
            Some(PowerProfile::Performance)
        );
    }

    #[test]
    fn windows_scheme_is_matched_by_guid_not_by_localized_name() {
        let output = "GUID do Esquema de Energia: A1841308-3541-4FAB-BC81-F71556F20B4A  (Economia de energia)\n";

        assert_eq!(
            parse_windows_active_scheme(output),
            Some(PowerProfile::PowerSaver)
        );
    }

    #[test]
    fn a_custom_windows_scheme_is_unknown() {
        let output = "Power Scheme GUID: 11111111-2222-3333-4444-555555555555  (My plan)\n";

        assert_eq!(parse_windows_active_scheme(output), None);
        assert_eq!(parse_windows_active_scheme(""), None);
    }
}
