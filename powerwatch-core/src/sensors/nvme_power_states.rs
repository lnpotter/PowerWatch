use crate::model::{Component, Confidence, PowerSensor, SensorError, SensorReading};

pub trait CurrentPowerStateSource: Send {
    fn read(&mut self) -> Result<u32, SensorError>;
}

pub struct NvmePowerStateDiskSensor<S: CurrentPowerStateSource> {
    power_states: Vec<PowerState>,
    source: S,
    device_name: String,
}

impl<S: CurrentPowerStateSource> NvmePowerStateDiskSensor<S> {
    pub fn new(power_states: Vec<PowerState>, source: S, device_name: String) -> Self {
        Self {
            power_states,
            source,
            device_name,
        }
    }
}

impl<S: CurrentPowerStateSource> PowerSensor for NvmePowerStateDiskSensor<S> {
    fn sample(&mut self) -> Result<SensorReading, SensorError> {
        let current_index = self.source.read()?;

        let state = self
            .power_states
            .iter()
            .find(|s| s.index == current_index)
            .ok_or_else(|| {
                SensorError::ReadFailed(format!(
                    "current power state {current_index} not found in the drive's power state table"
                ))
            })?;

        Ok(SensorReading {
            component: Component::Disk(self.device_name.clone()),
            watts: state.max_watts,
            confidence: Confidence::Estimated,
            timestamp: chrono::Utc::now(),
        })
    }
}
use std::path::Path;

pub fn read_current_power_state(path: &Path) -> Result<u32, SensorError> {
    let raw = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            SensorError::Unavailable(format!("{} does not exist", path.display()))
        }
        std::io::ErrorKind::PermissionDenied => {
            SensorError::PermissionDenied(format!("no read access to {}", path.display()))
        }
        _ => SensorError::ReadFailed(e.to_string()),
    })?;

    raw.trim()
        .parse::<u32>()
        .map_err(|e| SensorError::ReadFailed(format!("invalid power state value: {e}")))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerState {
    pub index: u32,
    pub max_watts: f64,
    pub operational: bool,
}

pub fn parse_nvme_power_states(text: &str) -> Vec<PowerState> {
    text.lines().filter_map(parse_power_state_line).collect()
}

fn parse_power_state_line(line: &str) -> Option<PowerState> {
    let line = line.trim();
    if !line.starts_with("ps") {
        return None;
    }

    let (before_colon, after_colon) = line.split_once(':')?;
    let index = before_colon
        .trim_start_matches("ps")
        .trim()
        .parse::<u32>()
        .ok()?;

    let mp_start = after_colon.find("mp:")? + "mp:".len();
    let after_mp = &after_colon[mp_start..];
    let watts_text = after_mp.split('W').next()?;
    let max_watts = watts_text.trim().parse::<f64>().ok()?;

    let operational = after_colon.contains(" operational");

    Some(PowerState {
        index,
        max_watts,
        operational,
    })
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{
        parse_nvme_power_states, read_current_power_state, CurrentPowerStateSource, PowerState,
    };
    use crate::model::SensorError;
    use std::path::PathBuf;

    pub fn fetch_power_state_table(device_path: &str) -> Result<Vec<PowerState>, SensorError> {
        let output = std::process::Command::new("nvme")
            .args(["id-ctrl", device_path])
            .output()
            .map_err(|e| SensorError::Unavailable(format!("failed to run nvme-cli: {e}")))?;

        if !output.status.success() {
            return Err(SensorError::Unavailable(format!(
                "nvme id-ctrl failed (is nvme-cli installed, and do you have permission?): {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let states = parse_nvme_power_states(&text);

        if states.is_empty() {
            return Err(SensorError::Unavailable(
                "no power states found in nvme id-ctrl output".to_string(),
            ));
        }

        Ok(states)
    }

    pub struct RealCurrentPowerStateSource {
        path: PathBuf,
    }

    impl RealCurrentPowerStateSource {
        pub fn new(path: PathBuf) -> Self {
            Self { path }
        }
    }

    impl CurrentPowerStateSource for RealCurrentPowerStateSource {
        fn read(&mut self) -> Result<u32, SensorError> {
            read_current_power_state(&self.path)
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::{fetch_power_state_table, RealCurrentPowerStateSource};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SAMPLE_ID_CTRL_OUTPUT: &str = "\
NVME Identify Controller:
vid     : 0x144d
mn      : Samsung SSD 970 EVO
npss    : 4
ps    0 : mp:7.60W operational enlat:0 exlat:0 rrt:0 rrl:0 rwt:0 rwl:0 idle_power:- active_power:-
ps    1 : mp:6.00W operational enlat:0 exlat:0 rrt:1 rrl:1 rwt:1 rwl:1 idle_power:- active_power:-
ps    2 : mp:3.80W operational enlat:0 exlat:0 rrt:2 rrl:2 rwt:2 rwl:2 idle_power:- active_power:-
ps    3 : mp:0.0700W non-operational enlat:2000 exlat:2000 rrt:3 rrl:3 rwt:3 rwl:3 idle_power:- active_power:-
";

    #[test]
    fn parses_every_power_state_line() {
        let states = parse_nvme_power_states(SAMPLE_ID_CTRL_OUTPUT);

        assert_eq!(states.len(), 4);
    }

    #[test]
    fn extracts_index_and_max_watts_for_an_operational_state() {
        let states = parse_nvme_power_states(SAMPLE_ID_CTRL_OUTPUT);

        assert_eq!(
            states[0],
            PowerState {
                index: 0,
                max_watts: 7.60,
                operational: true,
            }
        );
    }

    #[test]
    fn recognizes_a_non_operational_low_power_state() {
        let states = parse_nvme_power_states(SAMPLE_ID_CTRL_OUTPUT);

        assert_eq!(
            states[3],
            PowerState {
                index: 3,
                max_watts: 0.07,
                operational: false,
            }
        );
    }

    #[test]
    fn ignores_lines_that_are_not_power_states() {
        let states = parse_nvme_power_states("vid : 0x144d\nmn : Samsung SSD\n");

        assert!(states.is_empty());
    }

    #[test]
    fn handles_real_world_formatting_variants() {
        // different drives/nvme-cli versions pad the index differently
        let text = "ps 0 : mp:25.00W operational enlat:0 exlat:0\nps 1 : mp:9.00W operational enlat:0 exlat:0\n";

        let states = parse_nvme_power_states(text);

        assert_eq!(states.len(), 2);
        assert_eq!(states[0].max_watts, 25.0);
    }

    #[test]
    fn reads_the_current_power_state_number() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "2").unwrap();

        let state = read_current_power_state(file.path()).unwrap();

        assert_eq!(state, 2);
    }

    #[test]
    fn reports_unavailable_when_the_sysfs_path_is_missing() {
        let missing = std::path::Path::new("/tmp/powerwatch-test-missing-nvme-power-state");

        let error = read_current_power_state(missing).unwrap_err();

        assert!(matches!(error, SensorError::Unavailable(_)));
    }

    fn sample_power_states() -> Vec<PowerState> {
        vec![
            PowerState {
                index: 0,
                max_watts: 7.60,
                operational: true,
            },
            PowerState {
                index: 3,
                max_watts: 0.07,
                operational: false,
            },
        ]
    }

    struct FakeCurrentPowerStateSource {
        state: u32,
    }

    impl CurrentPowerStateSource for FakeCurrentPowerStateSource {
        fn read(&mut self) -> Result<u32, SensorError> {
            Ok(self.state)
        }
    }

    #[test]
    fn reports_the_max_watts_of_the_current_power_state() {
        let source = FakeCurrentPowerStateSource { state: 0 };
        let mut sensor =
            NvmePowerStateDiskSensor::new(sample_power_states(), source, "nvme0".to_string());

        let reading = sensor.sample().unwrap();

        assert_eq!(reading.watts, 7.60);
        assert_eq!(reading.confidence, Confidence::Estimated);
        assert_eq!(reading.component, Component::Disk("nvme0".to_string()));
    }

    #[test]
    fn reflects_a_different_current_state_on_the_next_sample() {
        let source = FakeCurrentPowerStateSource { state: 3 };
        let mut sensor =
            NvmePowerStateDiskSensor::new(sample_power_states(), source, "nvme0".to_string());

        let reading = sensor.sample().unwrap();

        assert_eq!(reading.watts, 0.07);
    }

    #[test]
    fn errors_clearly_when_the_current_state_is_not_in_the_table() {
        let source = FakeCurrentPowerStateSource { state: 99 };
        let mut sensor =
            NvmePowerStateDiskSensor::new(sample_power_states(), source, "nvme0".to_string());

        let result = sensor.sample();

        assert!(matches!(result.unwrap_err(), SensorError::ReadFailed(_)));
    }
}
