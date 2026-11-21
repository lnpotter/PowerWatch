use powerwatch_core::model::{Component, Confidence, SensorReading};
use powerwatch_core::sampler::Snapshot;
use powerwatch_core::suggestions::{
    ActionDescriptor, ActionKind, PowerProfile, Severity, SuggestionEngine, SuggestionRule,
};
use std::time::Duration;
use std::time::Instant;

fn cpu_snapshot(watts: f64) -> Snapshot {
    Snapshot {
        timestamp: chrono::Utc::now(),
        results: vec![(
            "cpu".to_string(),
            Ok(SensorReading {
                component: Component::Cpu,
                watts,
                confidence: Confidence::Measured,
                timestamp: chrono::Utc::now(),
            }),
        )],
    }
}

fn total_snapshot(watts: f64) -> Snapshot {
    Snapshot {
        timestamp: chrono::Utc::now(),
        results: vec![(
            "total".to_string(),
            Ok(SensorReading {
                component: Component::Total,
                watts,
                confidence: Confidence::Measured,
                timestamp: chrono::Utc::now(),
            }),
        )],
    }
}

#[test]
fn suggestion_engine_fires_on_sustained_high_cpu() {
    let mut engine = SuggestionEngine::new(vec![SuggestionRule {
        component_name: "cpu".to_string(),
        threshold_watts: 10.0,
        sustained_for: Duration::from_secs(2),
        message: "CPU high".to_string(),
        severity: Severity::Warning,
        action: Some(ActionDescriptor {
            label: "Switch to power saver".to_string(),
            kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
        }),
    }]);

    let t0 = Instant::now();

    let suggestions = engine.evaluate_at(&cpu_snapshot(5.0), t0);
    assert!(suggestions.is_empty(), "should not suggest below threshold");

    let suggestions = engine.evaluate_at(&cpu_snapshot(25.0), t0);
    assert!(
        suggestions.is_empty(),
        "should not suggest before sustained period"
    );

    let suggestions = engine.evaluate_at(&cpu_snapshot(25.0), t0 + Duration::from_secs(2));
    assert_eq!(
        suggestions.len(),
        1,
        "should fire suggestion after sustained period"
    );
    assert_eq!(suggestions[0].message, "CPU high");
    assert!(suggestions[0].action.is_some(), "should have action");

    let suggestions = engine.evaluate_at(&cpu_snapshot(25.0), t0 + Duration::from_secs(4));
    assert!(suggestions.is_empty(), "should not repeat while sustained");
}

#[test]
fn suggestion_engine_fires_on_sustained_total() {
    let mut engine = SuggestionEngine::new(vec![SuggestionRule {
        component_name: "total".to_string(),
        threshold_watts: 20.0,
        sustained_for: Duration::from_secs(1), // shortened for test
        message: "Total power high".to_string(),
        severity: Severity::Warning,
        action: None, // informational only
    }]);

    let t0 = Instant::now();

    let suggestions = engine.evaluate_at(&total_snapshot(10.0), t0);
    assert!(suggestions.is_empty());

    let suggestions = engine.evaluate_at(&total_snapshot(30.0), t0);
    assert!(
        suggestions.is_empty(),
        "should not fire before sustained period"
    );

    let suggestions = engine.evaluate_at(&total_snapshot(30.0), t0 + Duration::from_secs(2));
    assert_eq!(suggestions.len(), 1, "should fire after sustained period");
    assert_eq!(suggestions[0].message, "Total power high");
    assert!(
        suggestions[0].action.is_none(),
        "informational suggestion has no action"
    );
}

#[test]
fn multiple_rules_evaluate_independently() {
    let mut engine = SuggestionEngine::new(vec![
        SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: 10.0,
            sustained_for: Duration::from_secs(1),
            message: "CPU high".to_string(),
            severity: Severity::Warning,
            action: None,
        },
        SuggestionRule {
            component_name: "ram".to_string(),
            threshold_watts: 5.0,
            sustained_for: Duration::from_secs(1),
            message: "RAM high".to_string(),
            severity: Severity::Info,
            action: None,
        },
    ]);

    let t0 = Instant::now();

    let snapshot = Snapshot {
        timestamp: chrono::Utc::now(),
        results: vec![
            (
                "cpu".to_string(),
                Ok(SensorReading {
                    component: Component::Cpu,
                    watts: 5.0,
                    confidence: Confidence::Measured,
                    timestamp: chrono::Utc::now(),
                }),
            ),
            (
                "ram".to_string(),
                Ok(SensorReading {
                    component: Component::Ram,
                    watts: 2.0,
                    confidence: Confidence::Measured,
                    timestamp: chrono::Utc::now(),
                }),
            ),
        ],
    };

    let suggestions = engine.evaluate_at(&snapshot, t0);
    assert!(suggestions.is_empty(), "no suggestions below threshold");

    let snapshot_high = Snapshot {
        timestamp: chrono::Utc::now(),
        results: vec![
            (
                "cpu".to_string(),
                Ok(SensorReading {
                    component: Component::Cpu,
                    watts: 20.0,
                    confidence: Confidence::Measured,
                    timestamp: chrono::Utc::now(),
                }),
            ),
            (
                "ram".to_string(),
                Ok(SensorReading {
                    component: Component::Ram,
                    watts: 8.0,
                    confidence: Confidence::Measured,
                    timestamp: chrono::Utc::now(),
                }),
            ),
        ],
    };

    let suggestions = engine.evaluate_at(&snapshot_high, t0);
    assert!(
        suggestions.is_empty(),
        "should not suggest before sustained period"
    );

    let suggestions = engine.evaluate_at(&snapshot_high, t0 + Duration::from_secs(2));
    assert_eq!(suggestions.len(), 2, "should fire both rules");
    let messages: Vec<&str> = suggestions.iter().map(|s| s.message.as_str()).collect();
    assert!(messages.contains(&"CPU high"));
    assert!(messages.contains(&"RAM high"));
}
