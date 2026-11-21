use crate::model::{Component, Confidence, PowerSensor, SensorError, SensorReading};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

type Reply = Result<f64, SensorError>;

pub trait LhmPowerSource: Send {
    fn read_cpu_package_watts(&mut self) -> Result<f64, SensorError>;
}

pub struct LibreHardwareMonitorCpuSensor<S: LhmPowerSource> {
    source: S,
}

impl<S: LhmPowerSource> LibreHardwareMonitorCpuSensor<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: LhmPowerSource> PowerSensor for LibreHardwareMonitorCpuSensor<S> {
    fn sample(&mut self) -> Result<SensorReading, SensorError> {
        let watts = self.source.read_cpu_package_watts()?;

        Ok(SensorReading {
            component: Component::Cpu,
            watts,
            confidence: Confidence::Measured,
            timestamp: chrono::Utc::now(),
        })
    }
}

pub struct ThreadedLhmSource {
    requests: Sender<Sender<Reply>>,
    timeout: Duration,
}

impl ThreadedLhmSource {
    pub fn spawn<F, Q>(setup: F) -> Result<Self, SensorError>
    where
        F: FnOnce() -> Result<Q, SensorError> + Send + 'static,
        Q: FnMut() -> Reply,
    {
        Self::spawn_with_timeout(setup, QUERY_TIMEOUT)
    }

    pub fn spawn_with_timeout<F, Q>(setup: F, timeout: Duration) -> Result<Self, SensorError>
    where
        F: FnOnce() -> Result<Q, SensorError> + Send + 'static,
        Q: FnMut() -> Reply,
    {
        let (requests, incoming) = channel::<Sender<Reply>>();
        let (ready, ready_signal) = channel::<Result<(), SensorError>>();

        thread::Builder::new()
            .name("lhm-source".to_string())
            .spawn(move || match setup() {
                Ok(mut query) => {
                    let _ = ready.send(Ok(()));
                    for reply in incoming {
                        let _ = reply.send(query());
                    }
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            })
            .map_err(|e| {
                SensorError::Unavailable(format!("cannot start the source thread: {e}"))
            })?;

        ready_signal.recv().map_err(|_| {
            SensorError::Unavailable("the source thread stopped during setup".to_string())
        })??;

        Ok(Self { requests, timeout })
    }
}

fn stopped() -> SensorError {
    SensorError::ReadFailed("the source thread has stopped".to_string())
}

impl LhmPowerSource for ThreadedLhmSource {
    fn read_cpu_package_watts(&mut self) -> Result<f64, SensorError> {
        let (reply, answer) = channel();
        self.requests.send(reply).map_err(|_| stopped())?;

        match answer.recv_timeout(self.timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                Err(SensorError::ReadFailed("the query timed out".to_string()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    use super::{LhmPowerSource, Reply, ThreadedLhmSource};
    use crate::model::SensorError;
    use serde::Deserialize;
    use wmi::{COMLibrary, WMIConnection};

    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct SensorRow {
        value: f32,
    }

    pub struct RealLhmPowerSource {
        source: ThreadedLhmSource,
    }

    impl RealLhmPowerSource {
        pub fn new() -> Result<Self, SensorError> {
            Ok(Self {
                source: ThreadedLhmSource::spawn(connect)?,
            })
        }
    }

    impl LhmPowerSource for RealLhmPowerSource {
        fn read_cpu_package_watts(&mut self) -> Result<f64, SensorError> {
            self.source.read_cpu_package_watts()
        }
    }

    fn connect() -> Result<impl FnMut() -> Reply, SensorError> {
        let com = COMLibrary::new()
            .map_err(|e| SensorError::Unavailable(format!("COM initialization failed: {e}")))?;
        let conn =
            WMIConnection::with_namespace_path("ROOT\\LibreHardwareMonitor", com).map_err(|e| {
                SensorError::Unavailable(format!(
                    "LibreHardwareMonitor WMI namespace not found - is it running? ({e})"
                ))
            })?;

        Ok(move || query_cpu_package_watts(&conn))
    }

    fn query_cpu_package_watts(conn: &WMIConnection) -> Reply {
        let results: Vec<SensorRow> = conn
            .raw_query(
                "SELECT Value FROM Sensor WHERE SensorType = 'Power' AND Name LIKE '%CPU Package%'",
            )
            .map_err(|e| SensorError::ReadFailed(format!("WMI query failed: {e}")))?;

        results
            .into_iter()
            .next()
            .map(|row| f64::from(row.value))
            .ok_or_else(|| {
                SensorError::Unavailable(
                    "no 'CPU Package' power sensor found in LibreHardwareMonitor".to_string(),
                )
            })
    }
}

#[cfg(target_os = "windows")]
pub use win32::RealLhmPowerSource;

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLhmPowerSource {
        watts: f64,
    }

    impl LhmPowerSource for FakeLhmPowerSource {
        fn read_cpu_package_watts(&mut self) -> Result<f64, SensorError> {
            Ok(self.watts)
        }
    }

    #[test]
    fn reports_a_measured_reading_from_the_source() {
        let source = FakeLhmPowerSource { watts: 42.5 };
        let mut sensor = LibreHardwareMonitorCpuSensor::new(source);

        let reading = sensor.sample().unwrap();

        assert_eq!(reading.component, Component::Cpu);
        assert_eq!(reading.confidence, Confidence::Measured);
        assert_eq!(reading.watts, 42.5);
    }

    struct FailingLhmPowerSource;

    impl LhmPowerSource for FailingLhmPowerSource {
        fn read_cpu_package_watts(&mut self) -> Result<f64, SensorError> {
            Err(SensorError::Unavailable(
                "LibreHardwareMonitor not running".to_string(),
            ))
        }
    }

    #[test]
    fn propagates_source_errors_without_panicking() {
        let mut sensor = LibreHardwareMonitorCpuSensor::new(FailingLhmPowerSource);

        let result = sensor.sample();

        assert!(matches!(result.unwrap_err(), SensorError::Unavailable(_)));
    }

    fn assert_send<T: Send>() {}

    #[test]
    fn the_threaded_source_is_send_even_when_the_state_it_wraps_is_not() {
        use std::cell::Cell;
        use std::rc::Rc;

        assert_send::<ThreadedLhmSource>();

        let mut source = ThreadedLhmSource::spawn(|| {
            let watts = Rc::new(Cell::new(40.0));
            Ok(move || Ok(watts.get()))
        })
        .unwrap();

        assert_eq!(source.read_cpu_package_watts().unwrap(), 40.0);
    }

    #[test]
    fn a_setup_error_comes_back_from_spawn() {
        let result = ThreadedLhmSource::spawn(|| {
            Err::<Box<dyn FnMut() -> Reply>, _>(SensorError::Unavailable(
                "LibreHardwareMonitor not running".to_string(),
            ))
        });

        match result {
            Err(SensorError::Unavailable(message)) => {
                assert!(message.contains("not running"), "{message}")
            }
            _ => panic!("expected the setup error"),
        }
    }

    #[test]
    fn every_read_runs_on_the_same_worker_thread() {
        use std::sync::{Arc, Mutex};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let mut source = ThreadedLhmSource::spawn(move || {
            Ok(move || {
                recorder.lock().unwrap().push(thread::current().id());
                Ok(1.0)
            })
        })
        .unwrap();

        for _ in 0..3 {
            source.read_cpu_package_watts().unwrap();
        }

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|id| *id == seen[0]));
        assert_ne!(seen[0], thread::current().id());
    }

    #[test]
    fn query_errors_are_passed_through() {
        let mut source = ThreadedLhmSource::spawn(|| {
            Ok(|| Err(SensorError::ReadFailed("WMI query failed".to_string())))
        })
        .unwrap();

        assert!(matches!(
            source.read_cpu_package_watts(),
            Err(SensorError::ReadFailed(message)) if message.contains("WMI query failed")
        ));
    }

    #[test]
    fn a_slow_query_times_out_instead_of_blocking_forever() {
        let mut source = ThreadedLhmSource::spawn_with_timeout(
            || {
                Ok(|| {
                    thread::sleep(Duration::from_millis(400));
                    Ok(1.0)
                })
            },
            Duration::from_millis(50),
        )
        .unwrap();

        assert!(matches!(
            source.read_cpu_package_watts(),
            Err(SensorError::ReadFailed(message)) if message.contains("timed out")
        ));
    }

    #[test]
    fn a_worker_that_died_reports_an_error_instead_of_hanging() {
        let mut source =
            ThreadedLhmSource::spawn(|| Ok(|| -> Reply { panic!("simulated worker failure") }))
                .unwrap();

        assert!(source.read_cpu_package_watts().is_err());
        assert!(source.read_cpu_package_watts().is_err());
    }

    #[test]
    fn dropping_the_source_stops_the_worker_and_releases_its_state() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let released = Arc::new(AtomicBool::new(false));
        let flag = released.clone();
        let source = ThreadedLhmSource::spawn(move || {
            let state = DropFlag(flag);
            Ok(move || {
                let _keep = &state;
                Ok(1.0)
            })
        })
        .unwrap();

        drop(source);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !released.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(released.load(Ordering::SeqCst));
    }
}
