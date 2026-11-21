use powerwatch_core::sampler::{Sampler, Snapshot};
use powerwatch_core::storage::Storage;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub latest_snapshot: Arc<RwLock<Snapshot>>,
    pub storage: Arc<Mutex<Option<Storage>>>,
    pub suggestions: super::suggestions::SuggestionsState,
}

pub fn start_sampling_loop(
    mut sampler: Sampler,
    interval: Duration,
    storage: Option<Storage>,
    log_continuously: bool,
) -> AppState {
    let initial = sampler.sample_all();
    let latest_snapshot = Arc::new(RwLock::new(initial));
    let storage = Arc::new(Mutex::new(storage));
    let suggestions = super::suggestions::SuggestionsState::new();
    let state = AppState {
        latest_snapshot: latest_snapshot.clone(),
        storage: storage.clone(),
        suggestions: suggestions.clone(),
    };

    thread::spawn(move || loop {
        thread::sleep(interval);
        let snapshot = sampler.sample_all();

        suggestions.evaluate(&snapshot);

        if log_continuously {
            if let Ok(guard) = storage.lock() {
                if let Some(db) = guard.as_ref() {
                    for reading in snapshot.readings() {
                        let _ = db.insert_reading(reading);
                    }
                    if let Some(total) = snapshot.total() {
                        let _ = db.insert_reading(&total);
                    }
                }
            }
        }

        if let Ok(mut guard) = latest_snapshot.write() {
            *guard = snapshot;
        }
    });

    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerwatch_core::model::{Component, Confidence, PowerSensor, SensorError, SensorReading};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    fn reading(watts: f64) -> SensorReading {
        SensorReading {
            component: Component::Cpu,
            watts,
            confidence: Confidence::Measured,
            timestamp: chrono::Utc::now(),
        }
    }

    struct FixedSensor {
        reading: SensorReading,
    }

    impl PowerSensor for FixedSensor {
        fn sample(&mut self) -> Result<SensorReading, SensorError> {
            Ok(self.reading.clone())
        }
    }

    struct CountingSensor {
        calls: Arc<AtomicU32>,
    }

    impl PowerSensor for CountingSensor {
        fn sample(&mut self) -> Result<SensorReading, SensorError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(reading(n as f64))
        }
    }

    struct TimestampedSensor {
        calls: Arc<Mutex<Vec<Instant>>>,
    }

    impl PowerSensor for TimestampedSensor {
        fn sample(&mut self) -> Result<SensorReading, SensorError> {
            self.calls.lock().unwrap().push(Instant::now());
            Ok(reading(1.0))
        }
    }

    #[test]
    fn the_returned_state_has_a_valid_reading_immediately() {
        let mut sampler = Sampler::new();
        sampler.add_sensor(
            "cpu",
            Box::new(FixedSensor {
                reading: reading(42.0),
            }),
        );

        let state = start_sampling_loop(sampler, Duration::from_secs(60), None, false);

        let snapshot = state.latest_snapshot.read().unwrap();
        assert_eq!(snapshot.results.len(), 1);
        assert_eq!(snapshot.results[0].1.as_ref().unwrap().watts, 42.0);
    }

    #[test]
    fn the_background_thread_keeps_sampling_over_time() {
        let calls = Arc::new(AtomicU32::new(0));
        let mut sampler = Sampler::new();
        sampler.add_sensor(
            "cpu",
            Box::new(CountingSensor {
                calls: calls.clone(),
            }),
        );

        let state = start_sampling_loop(sampler, Duration::from_millis(20), None, false);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_second_sample = false;
        while Instant::now() < deadline {
            let watts = state.latest_snapshot.read().unwrap().results[0]
                .1
                .as_ref()
                .unwrap()
                .watts;
            if watts >= 1.0 {
                saw_second_sample = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            saw_second_sample,
            "expected the background loop to sample more than once"
        );
    }

    #[test]
    fn writes_to_storage_when_log_continuously_is_enabled() {
        let storage = Storage::open_in_memory().unwrap();
        let mut sampler = Sampler::new();
        sampler.add_sensor(
            "cpu",
            Box::new(FixedSensor {
                reading: reading(7.0),
            }),
        );

        let state = start_sampling_loop(sampler, Duration::from_millis(20), Some(storage), true);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut found_a_reading = false;
        while Instant::now() < deadline {
            let guard = state.storage.lock().unwrap();
            let db = guard.as_ref().unwrap();
            let readings = db
                .readings_since(chrono::Utc::now() - chrono::Duration::minutes(1))
                .unwrap();
            if !readings.is_empty() {
                found_a_reading = true;
                break;
            }
            drop(guard);
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            found_a_reading,
            "expected a reading to be written to storage"
        );
    }

    #[test]
    fn does_not_write_to_storage_when_log_continuously_is_disabled() {
        let storage = Storage::open_in_memory().unwrap();
        let mut sampler = Sampler::new();
        sampler.add_sensor(
            "cpu",
            Box::new(FixedSensor {
                reading: reading(7.0),
            }),
        );

        let state = start_sampling_loop(sampler, Duration::from_millis(20), Some(storage), false);
        thread::sleep(Duration::from_millis(200));

        let guard = state.storage.lock().unwrap();
        let db = guard.as_ref().unwrap();
        let readings = db
            .readings_since(chrono::Utc::now() - chrono::Duration::minutes(1))
            .unwrap();

        assert!(readings.is_empty());
    }

    #[test]
    fn samples_once_per_interval() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut sampler = Sampler::new();
        sampler.add_sensor(
            "cpu",
            Box::new(TimestampedSensor {
                calls: calls.clone(),
            }),
        );

        let interval = Duration::from_millis(50);
        let _state = start_sampling_loop(sampler, interval, None, false);
        thread::sleep(Duration::from_millis(400));

        let calls = calls.lock().unwrap();
        assert!(
            calls.len() >= 3,
            "expected several samples, got {}",
            calls.len()
        );
        let min_gap = calls.windows(2).map(|w| w[1] - w[0]).min().unwrap();
        assert!(
            min_gap >= interval / 2,
            "samples were taken {min_gap:?} apart, expected about {interval:?}"
        );
    }
}