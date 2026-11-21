use crate::suggestions::{ActionKind, PowerProfile};

#[derive(Debug)]
pub enum ActionError {
    Unsupported(String),
    ExecutionFailed(String),
}

pub fn linux_command_for(profile: PowerProfile) -> (&'static str, Vec<String>) {
    let mode = match profile {
        PowerProfile::PowerSaver => "power-saver",
        PowerProfile::Balanced => "balanced",
        PowerProfile::Performance => "performance",
    };
    (
        "powerprofilesctl",
        vec!["set".to_string(), mode.to_string()],
    )
}

pub fn windows_command_for(profile: PowerProfile) -> (&'static str, Vec<String>) {
    let scheme = match profile {
        PowerProfile::PowerSaver => "SCHEME_MAX",
        PowerProfile::Balanced => "SCHEME_BALANCED",
        PowerProfile::Performance => "SCHEME_MIN",
    };
    (
        "powercfg",
        vec!["/setactive".to_string(), scheme.to_string()],
    )
}

pub fn macos_command_for(profile: PowerProfile) -> (&'static str, Vec<String>) {
    let value = match profile {
        PowerProfile::PowerSaver => "1",
        PowerProfile::Balanced | PowerProfile::Performance => "0",
    };
    (
        "pmset",
        vec![
            "-a".to_string(),
            "lowpowermode".to_string(),
            value.to_string(),
        ],
    )
}

pub fn linux_command_for_top_processes() -> (&'static str, Vec<String>) {
    (
        "sh",
        vec![
            "-c".to_string(),
            "ps -eo pid,pcpu,pmem,rss,comm --sort=-%cpu | head -6".to_string(),
        ],
    )
}

pub fn macos_command_for_top_processes() -> (&'static str, Vec<String>) {
    (
        "sh",
        vec![
            "-c".to_string(),
            "ps -eo pid,pcpu,pmem,rss,comm -r | head -6".to_string(),
        ],
    )
}

pub fn windows_command_for_top_processes() -> (&'static str, Vec<String>) {
    (
        "powershell",
        vec![
            "-command".to_string(),
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 5 Id,CPU,WorkingSet,ProcessName | Format-Table -AutoSize".to_string(),
        ],
    )
}

pub trait CommandRunner: Send {
    fn run(&mut self, program: &str, args: &[String]) -> Result<(), ActionError>;
}

pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<(), ActionError> {
        let status = std::process::Command::new(program)
            .args(args)
            .status()
            .map_err(|e| ActionError::ExecutionFailed(format!("failed to run {program}: {e}")))?;

        if !status.success() {
            return Err(ActionError::ExecutionFailed(format!(
                "{program} exited with a non-zero status"
            )));
        }

        Ok(())
    }
}

pub struct PowerProfileActionExecutor<R: CommandRunner> {
    runner: R,
    command_for: fn(PowerProfile) -> (&'static str, Vec<String>),
    command_for_top_processes: fn() -> (&'static str, Vec<String>),
}

impl<R: CommandRunner> PowerProfileActionExecutor<R> {
    pub fn new(
        runner: R,
        command_for: fn(PowerProfile) -> (&'static str, Vec<String>),
        command_for_top_processes: fn() -> (&'static str, Vec<String>),
    ) -> Self {
        Self {
            runner,
            command_for,
            command_for_top_processes,
        }
    }

    pub fn apply(&mut self, action: &ActionKind) -> Result<(), ActionError> {
        match action {
            ActionKind::SetPowerProfile(profile) => {
                let (program, args) = (self.command_for)(*profile);
                self.runner.run(program, &args)
            }
            ActionKind::ShowTopProcesses => {
                let (program, args) = (self.command_for_top_processes)();
                self.runner.run(program, &args)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }

    pub fn profile_command(self, profile: PowerProfile) -> (&'static str, Vec<String>) {
        match self {
            Platform::Linux => linux_command_for(profile),
            Platform::MacOs => macos_command_for(profile),
            Platform::Windows => windows_command_for(profile),
        }
    }

    pub fn top_processes_command(self) -> (&'static str, Vec<String>) {
        match self {
            Platform::Linux => linux_command_for_top_processes(),
            Platform::MacOs => macos_command_for_top_processes(),
            Platform::Windows => windows_command_for_top_processes(),
        }
    }

    pub fn command_line(self, kind: &ActionKind) -> String {
        let (program, args) = match kind {
            ActionKind::SetPowerProfile(profile) => self.profile_command(*profile),
            ActionKind::ShowTopProcesses => self.top_processes_command(),
        };
        std::iter::once(program.to_string())
            .chain(args)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn profile_command_for_current_os(profile: PowerProfile) -> (&'static str, Vec<String>) {
    Platform::current().profile_command(profile)
}

fn top_processes_command_for_current_os() -> (&'static str, Vec<String>) {
    Platform::current().top_processes_command()
}

impl PowerProfileActionExecutor<RealCommandRunner> {
    pub fn for_current_os() -> Self {
        Self::new(
            RealCommandRunner,
            profile_command_for_current_os,
            top_processes_command_for_current_os,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_command_covers_every_profile() {
        assert_eq!(
            linux_command_for(PowerProfile::PowerSaver),
            (
                "powerprofilesctl",
                vec!["set".to_string(), "power-saver".to_string()]
            )
        );
        assert_eq!(
            linux_command_for(PowerProfile::Balanced),
            (
                "powerprofilesctl",
                vec!["set".to_string(), "balanced".to_string()]
            )
        );
        assert_eq!(
            linux_command_for(PowerProfile::Performance),
            (
                "powerprofilesctl",
                vec!["set".to_string(), "performance".to_string()]
            )
        );
    }

    #[test]
    fn windows_command_covers_every_profile() {
        assert_eq!(
            windows_command_for(PowerProfile::PowerSaver),
            (
                "powercfg",
                vec!["/setactive".to_string(), "SCHEME_MAX".to_string()]
            )
        );
        assert_eq!(
            windows_command_for(PowerProfile::Balanced),
            (
                "powercfg",
                vec!["/setactive".to_string(), "SCHEME_BALANCED".to_string()]
            )
        );
        assert_eq!(
            windows_command_for(PowerProfile::Performance),
            (
                "powercfg",
                vec!["/setactive".to_string(), "SCHEME_MIN".to_string()]
            )
        );
    }

    #[test]
    fn macos_command_covers_every_profile() {
        assert_eq!(
            macos_command_for(PowerProfile::PowerSaver),
            (
                "pmset",
                vec![
                    "-a".to_string(),
                    "lowpowermode".to_string(),
                    "1".to_string()
                ]
            )
        );
        assert_eq!(
            macos_command_for(PowerProfile::Balanced),
            (
                "pmset",
                vec![
                    "-a".to_string(),
                    "lowpowermode".to_string(),
                    "0".to_string()
                ]
            )
        );
        assert_eq!(
            macos_command_for(PowerProfile::Performance),
            (
                "pmset",
                vec![
                    "-a".to_string(),
                    "lowpowermode".to_string(),
                    "0".to_string()
                ]
            )
        );
    }

    #[test]
    fn fake_runner_does_not_execute() {
        struct Fake;
        impl CommandRunner for Fake {
            fn run(&mut self, _program: &str, _args: &[String]) -> Result<(), ActionError> {
                Ok(())
            }
        }
        let mut executor = PowerProfileActionExecutor::new(
            Fake,
            linux_command_for,
            linux_command_for_top_processes,
        );
        assert!(executor
            .apply(&ActionKind::SetPowerProfile(PowerProfile::PowerSaver))
            .is_ok());
    }

    #[test]
    fn failed_runner_propagates_error() {
        struct Failing;
        impl CommandRunner for Failing {
            fn run(&mut self, _program: &str, _args: &[String]) -> Result<(), ActionError> {
                Err(ActionError::ExecutionFailed("boom".to_string()))
            }
        }
        let mut executor = PowerProfileActionExecutor::new(
            Failing,
            linux_command_for,
            linux_command_for_top_processes,
        );
        assert!(executor
            .apply(&ActionKind::SetPowerProfile(PowerProfile::PowerSaver))
            .is_err());
    }

    #[test]
    fn top_processes_commands_are_defined() {
        assert_eq!(
            linux_command_for_top_processes(),
            (
                "sh",
                vec![
                    "-c".to_string(),
                    "ps -eo pid,pcpu,pmem,rss,comm --sort=-%cpu | head -6".to_string()
                ]
            )
        );
        assert_eq!(
            macos_command_for_top_processes(),
            (
                "sh",
                vec![
                    "-c".to_string(),
                    "ps -eo pid,pcpu,pmem,rss,comm -r | head -6".to_string()
                ]
            )
        );
        assert_eq!(
            windows_command_for_top_processes(),
            (
                "powershell",
                vec![
                    "-command".to_string(),
                    "Get-Process | Sort-Object CPU -Descending | Select-Object -First 5 Id,CPU,WorkingSet,ProcessName | Format-Table -AutoSize".to_string(),
                ],
            )
        );
    }

    #[test]
    fn platform_picks_the_commands_of_its_own_os() {
        let profile = PowerProfile::PowerSaver;

        assert_eq!(
            Platform::Linux.profile_command(profile),
            linux_command_for(profile)
        );
        assert_eq!(
            Platform::MacOs.profile_command(profile),
            macos_command_for(profile)
        );
        assert_eq!(
            Platform::Windows.profile_command(profile),
            windows_command_for(profile)
        );

        assert_eq!(
            Platform::MacOs.top_processes_command(),
            macos_command_for_top_processes()
        );
        assert_eq!(
            Platform::Windows.top_processes_command(),
            windows_command_for_top_processes()
        );
    }

    #[test]
    fn the_command_line_shows_exactly_what_will_run() {
        let power_saver = ActionKind::SetPowerProfile(PowerProfile::PowerSaver);

        assert_eq!(
            Platform::Linux.command_line(&power_saver),
            "powerprofilesctl set power-saver"
        );
        assert_eq!(
            Platform::MacOs.command_line(&power_saver),
            "pmset -a lowpowermode 1"
        );
        assert_eq!(
            Platform::Windows.command_line(&power_saver),
            "powercfg /setactive SCHEME_MAX"
        );
    }

    #[test]
    fn the_command_line_is_the_command_the_executor_runs() {
        use std::sync::{Arc, Mutex};

        struct Recorder(Arc<Mutex<Vec<String>>>);
        impl CommandRunner for Recorder {
            fn run(&mut self, program: &str, args: &[String]) -> Result<(), ActionError> {
                let line = std::iter::once(program.to_string())
                    .chain(args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" ");
                self.0.lock().unwrap().push(line);
                Ok(())
            }
        }

        for profile in [
            PowerProfile::PowerSaver,
            PowerProfile::Balanced,
            PowerProfile::Performance,
        ] {
            let ran = Arc::new(Mutex::new(Vec::new()));
            let mut executor = PowerProfileActionExecutor::new(
                Recorder(ran.clone()),
                linux_command_for,
                linux_command_for_top_processes,
            );
            let kind = ActionKind::SetPowerProfile(profile);

            executor.apply(&kind).unwrap();

            assert_eq!(
                *ran.lock().unwrap(),
                vec![Platform::Linux.command_line(&kind)]
            );
        }
    }

    #[test]
    fn current_platform_matches_the_build_target() {
        let expected = if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        };
        assert_eq!(Platform::current(), expected);
    }

    #[test]
    fn the_current_os_commands_use_the_right_program() {
        let (program, _) = profile_command_for_current_os(PowerProfile::PowerSaver);
        let expected = match Platform::current() {
            Platform::Linux => "powerprofilesctl",
            Platform::MacOs => "pmset",
            Platform::Windows => "powercfg",
        };
        assert_eq!(program, expected);
    }
}
