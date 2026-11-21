use std::process::Command;

#[derive(Debug)]
pub enum HookError {
    SpawnFailed(String),
}

pub fn run_hook(command: &str) -> Result<(), HookError> {
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    run_hook_with_shell(shell, flag, command)
}

fn run_hook_with_shell(shell: &str, flag: &str, command: &str) -> Result<(), HookError> {
    let child = Command::new(shell)
        .arg(flag)
        .arg(command)
        .spawn()
        .map_err(|e| HookError::SpawnFailed(e.to_string()))?;

    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for_file_content(path: &std::path::Path, expected: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(contents) = std::fs::read_to_string(path) {
                if contents.trim() == expected {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn runs_a_real_command_and_its_effect_shows_up() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let command = format!("echo hello > {}", path.display());

        run_hook(&command).unwrap();

        assert!(wait_for_file_content(
            &path,
            "hello",
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn does_not_block_while_the_command_is_still_running() {
        let start = Instant::now();

        run_hook("sleep 2").unwrap();

        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_missing_shell_is_reported_as_a_spawn_failure_not_a_panic() {
        let result = run_hook_with_shell("/no/such/shell-binary", "-c", "echo hi");

        assert!(matches!(result, Err(HookError::SpawnFailed(_))));
    }
}
