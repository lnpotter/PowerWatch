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
            "ps -eo pid,pcpu,pmem,rss,comm -m | head -6".to_string(),
        ],
    )
}

pub fn linux_command_for_sleep_timer() -> (&'static str, Vec<String>) {
    (
        "sh",
        vec![
            "-c".to_string(),
            "loginctl show-session -p IdleAction 2>/dev/null || echo 'IdleAction=none'".to_string(),
        ],
    )
}

pub fn macos_command_for_sleep_timer() -> (&'static str, Vec<String>) {
    ("sh", vec!["-c".to_string(), "pmset -g 2>/dev/null | grep -E 'sleep|disksleep|displaySleep' || echo 'Sleep settings not available'".to_string()])
}

pub fn windows_command_for_sleep_timer() -> (&'static str, Vec<String>) {
    (
        "powershell",
        vec![
            "-command".to_string(),
            "powercfg /getactivescheme".to_string(),
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
    command_for_sleep_timer: fn() -> (&'static str, Vec<String>),
}

impl<R: CommandRunner> PowerProfileActionExecutor<R> {
    pub fn new(
        runner: R,
        command_for: fn(PowerProfile) -> (&'static str, Vec<String>),
        command_for_top_processes: fn() -> (&'static str, Vec<String>),
        command_for_sleep_timer: fn() -> (&'static str, Vec<String>),
    ) -> Self {
        Self {
            runner,
            command_for,
            command_for_top_processes,
            command_for_sleep_timer,
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
            ActionKind::ShowSleepTimer => {
                let (program, args) = (self.command_for_sleep_timer)();
                self.runner.run(program, &args)
            }
        }
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
            linux_command_for_sleep_timer,
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
            linux_command_for_sleep_timer,
        );
        assert!(executor
            .apply(&ActionKind::SetPowerProfile(PowerProfile::PowerSaver))
            .is_err());
    }

    #[test]
    fn sleep_timer_commands_are_defined() {
        assert_eq!(
            linux_command_for_sleep_timer(),
            (
                "sh",
                vec![
                    "-c".to_string(),
                    "loginctl show-session -p IdleAction 2>/dev/null || echo 'IdleAction=none'"
                        .to_string()
                ]
            )
        );
        assert_eq!(
            macos_command_for_sleep_timer(),
            ("sh", vec!["-c".to_string(), "pmset -g 2>/dev/null | grep -E 'sleep|disksleep|displaySleep' || echo 'Sleep settings not available'".to_string()])
        );
        assert_eq!(
            windows_command_for_sleep_timer(),
            (
                "powershell",
                vec![
                    "-command".to_string(),
                    "powercfg /getactivescheme".to_string()
                ],
            )
        );
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
                    "ps -eo pid,pcpu,pmem,rss,comm -m | head -6".to_string()
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
}
