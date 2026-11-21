use crate::model::{Component, Confidence, PowerSensor, SensorError, SensorReading};
use crate::sensors::disk::DiskType;

pub trait DiskActivitySource: Send {
    fn read(&mut self) -> Result<u64, SensorError>;
}

pub struct WindowsDiskSensor<S: DiskActivitySource> {
    source: S,
    disk_type: DiskType,
    last_count: Option<u64>,
}

impl<S: DiskActivitySource> WindowsDiskSensor<S> {
    pub fn new(source: S, disk_type: DiskType) -> Self {
        Self {
            source,
            disk_type,
            last_count: None,
        }
    }
}

impl<S: DiskActivitySource> PowerSensor for WindowsDiskSensor<S> {
    fn sample(&mut self) -> Result<SensorReading, SensorError> {
        let count = self.source.read()?;
        let was_active = self.last_count.is_some_and(|last| count != last);
        self.last_count = Some(count);

        let watts = if was_active {
            self.disk_type.active_watts()
        } else {
            self.disk_type.idle_watts()
        };

        Ok(SensorReading {
            component: Component::Disk("Windows".to_string()),
            watts,
            confidence: Confidence::Estimated,
            timestamp: chrono::Utc::now(),
        })
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    use super::DiskActivitySource;
    use crate::model::SensorError;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Performance::{
        PdhAddCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetRawCounterValue, PdhOpenQueryW,
        PDH_RAW_COUNTER,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub struct WindowsDiskActivitySource {
        query: isize,
        counter: isize,
    }

    impl WindowsDiskActivitySource {
        pub fn new() -> Result<Self, SensorError> {
            let mut query: isize = 0;
            let mut counter: isize = 0;

            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                    return Err(SensorError::Unavailable("PdhOpenQuery failed".to_string()));
                }

                let path = to_wide(r"\PhysicalDisk(_Total)\Disk Transfers/sec");
                if PdhAddCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                    PdhCloseQuery(query);
                    return Err(SensorError::Unavailable(
                        "PdhAddCounter failed - no PhysicalDisk counter available".to_string(),
                    ));
                }
            }

            Ok(Self { query, counter })
        }
    }

    impl DiskActivitySource for WindowsDiskActivitySource {
        fn read(&mut self) -> Result<u64, SensorError> {
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return Err(SensorError::ReadFailed(
                        "PdhCollectQueryData failed".to_string(),
                    ));
                }

                let mut raw: PDH_RAW_COUNTER = std::mem::zeroed();
                let mut counter_type: u32 = 0;
                if PdhGetRawCounterValue(self.counter, &mut counter_type, &mut raw) != 0 {
                    return Err(SensorError::ReadFailed(
                        "PdhGetRawCounterValue failed".to_string(),
                    ));
                }

                Ok(raw.FirstValue as u64)
            }
        }
    }

    impl Drop for WindowsDiskActivitySource {
        fn drop(&mut self) {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    unsafe impl Send for WindowsDiskActivitySource {}
}

#[cfg(target_os = "windows")]
pub use win32::WindowsDiskActivitySource;

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeDiskActivitySource {
        counts: std::collections::VecDeque<u64>,
    }

    impl FakeDiskActivitySource {
        fn new(counts: Vec<u64>) -> Self {
            Self {
                counts: counts.into(),
            }
        }
    }

    impl DiskActivitySource for FakeDiskActivitySource {
        fn read(&mut self) -> Result<u64, SensorError> {
            self.counts
                .pop_front()
                .ok_or_else(|| SensorError::ReadFailed("no more fake readings".to_string()))
        }
    }

    #[test]
    fn first_sample_assumes_idle_since_theres_nothing_to_compare_to() {
        let source = FakeDiskActivitySource::new(vec![100]);
        let mut sensor = WindowsDiskSensor::new(source, DiskType::Nvme);

        let reading = sensor.sample().unwrap();

        assert_eq!(reading.watts, DiskType::Nvme.idle_watts());
        assert_eq!(reading.confidence, Confidence::Estimated);
        assert_eq!(reading.component, Component::Disk("Windows".to_string()));
    }

    #[test]
    fn reports_active_watts_when_the_counter_changed_between_samples() {
        let source = FakeDiskActivitySource::new(vec![100, 250]);
        let mut sensor = WindowsDiskSensor::new(source, DiskType::SsdSata);

        sensor.sample().unwrap();
        let reading = sensor.sample().unwrap();

        assert_eq!(reading.watts, DiskType::SsdSata.active_watts());
    }

    #[test]
    fn reports_idle_watts_when_the_counter_stayed_the_same() {
        let source = FakeDiskActivitySource::new(vec![100, 100]);
        let mut sensor = WindowsDiskSensor::new(source, DiskType::Hdd7200Rpm);

        sensor.sample().unwrap();
        let reading = sensor.sample().unwrap();

        assert_eq!(reading.watts, DiskType::Hdd7200Rpm.idle_watts());
    }
}
