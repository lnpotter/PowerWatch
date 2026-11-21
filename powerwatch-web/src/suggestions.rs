use powerwatch_core::actions::{
    linux_command_for, linux_command_for_sleep_timer, linux_command_for_top_processes, ActionError,
    PowerProfileActionExecutor, RealCommandRunner,
};
use powerwatch_core::sampler::Snapshot;
use powerwatch_core::suggestions::{
    ActionDescriptor, ActionKind, PowerProfile, Severity, Suggestion,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN_TTL: Duration = Duration::from_secs(60);

struct IssuedToken {
    component_name: String,
    issued: Instant,
}

#[derive(Clone)]
pub struct SuggestionsState {
    pub engine: Arc<Mutex<SuggestionEngine>>,
    pub active: Arc<Mutex<Vec<Suggestion>>>,
    tokens: Arc<Mutex<HashMap<String, IssuedToken>>>,
}

impl SuggestionsState {
    pub fn new() -> Self {
        Self {
            engine: Arc::new(Mutex::new(SuggestionEngine::default())),
            active: Arc::new(Mutex::new(Vec::new())),
            tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn evaluate(&self, snapshot: &Snapshot) {
        let mut engine = self.engine.lock().unwrap();
        let new_suggestions = engine.evaluate(snapshot);
        drop(engine);

        let mut active = self.active.lock().unwrap();
        active.retain(|s| {
            !new_suggestions
                .iter()
                .any(|ns| ns.component_name == s.component_name)
        });

        for s in new_suggestions {
            active.push(s);
        }
    }

    pub fn list_with_tokens(&self) -> Vec<Proposal> {
        let active = self.active.lock().unwrap();
        let mut tokens = self.tokens.lock().unwrap();
        let now = Instant::now();
        tokens.retain(|_, t| now.duration_since(t.issued) < TOKEN_TTL);

        active
            .iter()
            .map(|s| {
                let token = s.action.as_ref().map(|_| {
                    let existing = tokens
                        .iter()
                        .find(|(_, t)| t.component_name == s.component_name)
                        .map(|(token, _)| token.clone());
                    existing.unwrap_or_else(|| {
                        let token = new_token();
                        tokens.insert(
                            token.clone(),
                            IssuedToken {
                                component_name: s.component_name.clone(),
                                issued: now,
                            },
                        );
                        token
                    })
                });
                Proposal {
                    suggestion: s.clone(),
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
            .active
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.component_name == issued.component_name)
            .and_then(|s| s.action.clone())
            .ok_or_else(|| "suggestion is no longer active".to_string())?;

        run(&action)?;

        self.active
            .lock()
            .unwrap()
            .retain(|s| s.component_name != issued.component_name);
        Ok(true)
    }
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
    pub suggestion: Suggestion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Deserialize)]
pub struct ApplyRequest {
    pub token: String,
}

#[derive(Serialize)]
pub struct ApplyResponse {
    pub success: bool,
    pub message: String,
}

fn apply_action(action: &ActionDescriptor) -> Result<(), String> {
    let mut executor = PowerProfileActionExecutor::new(
        RealCommandRunner,
        linux_command_for,
        linux_command_for_top_processes,
        linux_command_for_sleep_timer,
    );
    executor.apply(&action.kind).map_err(|e| match e {
        ActionError::Unsupported(msg) | ActionError::ExecutionFailed(msg) => msg,
    })
}

#[derive(Default)]
pub struct SuggestionEngine {
    fired: std::collections::HashMap<String, (f64, Instant)>,
}

impl SuggestionEngine {
    pub fn evaluate(&mut self, snapshot: &Snapshot) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();
        let now = Instant::now();
        let current_profile = detect_current_profile();

        if let Some(total) = snapshot.total() {
            if total.watts > 15.0 {
                let key = "total_high".to_string();
                let entry = self.fired.entry(key.clone()).or_insert((0.0, now));
                if entry.0 < 15.0 {
                    entry.0 = total.watts;
                    entry.1 = now;
                } else if now.duration_since(entry.1) >= Duration::from_secs(3) {
                    if current_profile != Some(PowerProfile::PowerSaver) {
                        suggestions.push(Suggestion {
                            message: "Total power draw has been high. Consider switching to power saver profile.".to_string(),
                            component_name: "total".to_string(),
                            severity: Severity::Warning,
                            action: Some(ActionDescriptor {
                                label: "Switch to power saver".to_string(),
                                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
                            }),
                        });
                    }
                    self.fired.remove(&key);
                }
            } else {
                self.fired.remove("total_high");
            }
        }

        if let Some(cpu_reading) = snapshot
            .results
            .iter()
            .find(|(name, _)| name == "cpu")
            .and_then(|(_, r)| r.as_ref().ok())
        {
            if cpu_reading.watts > 8.0 {
                let key = "cpu_high".to_string();
                let entry = self.fired.entry(key.clone()).or_insert((0.0, now));
                if entry.0 < 8.0 {
                    entry.0 = cpu_reading.watts;
                    entry.1 = now;
                } else if now.duration_since(entry.1) >= Duration::from_secs(3) {
                    if current_profile != Some(PowerProfile::PowerSaver) {
                        suggestions.push(Suggestion {
                            message: "CPU power draw has been high. Consider reducing workload or switching to power saver.".to_string(),
                            component_name: "cpu".to_string(),
                            severity: Severity::Warning,
                            action: Some(ActionDescriptor {
                                label: "Switch to power saver".to_string(),
                                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
                            }),
                        });
                    }
                    self.fired.remove(&key);
                }
            } else {
                self.fired.remove("cpu_high");
            }
        }

        if let Some(cpu_reading) = snapshot
            .results
            .iter()
            .find(|(name, _)| name == "cpu")
            .and_then(|(_, r)| r.as_ref().ok())
        {
            if cpu_reading.watts > 8.0 {
                let key = "cpu_high_info".to_string();
                let entry = self.fired.entry(key.clone()).or_insert((0.0, now));
                if entry.0 < 8.0 {
                    entry.0 = cpu_reading.watts;
                    entry.1 = now;
                } else if now.duration_since(entry.1) >= Duration::from_secs(10) {
                    suggestions.push(Suggestion {
                        message: "CPU usage has been high for a while. Review top processes to identify what's consuming resources.".to_string(),
                        component_name: "cpu".to_string(),
                        severity: Severity::Info,
                        action: Some(ActionDescriptor {
                            label: "Show top processes".to_string(),
                            kind: ActionKind::ShowTopProcesses,
                        }),
                    });
                    self.fired.remove(&key);
                }
            } else {
                self.fired.remove("cpu_high_info");
            }
        }

        suggestions
    }
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
        // TODO
    }

    None
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

    #[test]
    fn suggestion_engine_fires_when_threshold_sustained() {
        let mut engine = SuggestionEngine::default();

        // below threshold
        let suggestions = engine.evaluate(&snapshot_with("cpu", 10.0));
        assert!(suggestions.is_empty());

        // cross threshold
        let suggestions = engine.evaluate(&snapshot_with("cpu", 40.0));
        assert!(suggestions.is_empty()); // not yet sustained
    }

    #[test]
    fn suggestions_state_lists_with_tokens() {
        let state = SuggestionsState::new();
        let suggestions = state.list_with_tokens();
        assert!(suggestions.is_empty());
    }

    fn suggestion(component: &str) -> Suggestion {
        Suggestion {
            message: component.to_string(),
            component_name: component.to_string(),
            severity: Severity::Info,
            action: Some(ActionDescriptor {
                label: component.to_string(),
                kind: ActionKind::ShowTopProcesses,
            }),
        }
    }

    fn state_with(components: &[&str]) -> SuggestionsState {
        let state = SuggestionsState::new();
        *state.active.lock().unwrap() = components.iter().map(|c| suggestion(c)).collect();
        state
    }

    fn token_for(state: &SuggestionsState, component: &str) -> String {
        state
            .list_with_tokens()
            .into_iter()
            .find(|p| p.suggestion.component_name == component)
            .and_then(|p| p.token)
            .unwrap()
    }

    fn remaining(state: &SuggestionsState) -> Vec<String> {
        let active = state.active.lock().unwrap();
        active.iter().map(|s| s.component_name.clone()).collect()
    }

    #[test]
    fn a_token_applies_only_its_own_suggestion() {
        let state = state_with(&["cpu", "total"]);
        let token = token_for(&state, "total");
        let mut ran = Vec::new();

        state
            .apply_with(&token, |action| {
                ran.push(action.label.clone());
                Ok(())
            })
            .unwrap();

        assert_eq!(ran, vec!["total"]);
        assert_eq!(remaining(&state), vec!["cpu"]);
    }

    #[test]
    fn a_failed_action_is_reported_and_the_suggestion_stays() {
        let state = state_with(&["cpu"]);
        let token = token_for(&state, "cpu");

        let result = state.apply_with(&token, |_| Err("powerprofilesctl not found".to_string()));

        assert_eq!(result, Err("powerprofilesctl not found".to_string()));
        assert_eq!(remaining(&state), vec!["cpu"]);
    }

    #[test]
    fn a_token_works_only_once() {
        let state = state_with(&["cpu"]);
        let token = token_for(&state, "cpu");

        assert!(state.apply_with(&token, |_| Ok(())).is_ok());
        assert!(state.apply_with(&token, |_| Ok(())).is_err());
    }

    #[test]
    fn an_unknown_token_runs_nothing() {
        let state = state_with(&["cpu"]);
        let mut ran = false;

        let result = state.apply_with("made-up", |_| {
            ran = true;
            Ok(())
        });

        assert!(result.is_err());
        assert!(!ran);
    }

    #[test]
    fn a_token_for_a_suggestion_that_is_gone_runs_nothing() {
        let state = state_with(&["cpu"]);
        let token = token_for(&state, "cpu");
        state.active.lock().unwrap().clear();
        let mut ran = false;

        let result = state.apply_with(&token, |_| {
            ran = true;
            Ok(())
        });

        assert!(result.is_err());
        assert!(!ran);
    }

    #[test]
    fn listing_again_reuses_the_token_instead_of_issuing_new_ones() {
        let state = state_with(&["cpu"]);

        let first = token_for(&state, "cpu");
        let second = token_for(&state, "cpu");

        assert_eq!(first, second);
        assert_eq!(state.tokens.lock().unwrap().len(), 1);
    }
}