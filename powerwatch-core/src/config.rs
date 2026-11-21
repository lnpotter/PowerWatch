use crate::suggestions::{ActionDescriptor, ActionKind, PowerProfile, Severity, SuggestionRule};
use serde::Deserialize;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const EXAMPLE_CONFIG: &str = include_str!("../../config.example.toml");

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub suggestions: SuggestionsConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuggestionsConfig {
    pub clear_after_seconds: u64,
    pub power_saver: PowerSaverConfig,
    pub top_processes: TopProcessesConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PowerSaverConfig {
    pub enabled: bool,
    pub total_watts: f64,
    pub cpu_watts: f64,
    pub sustained_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TopProcessesConfig {
    pub enabled: bool,
    pub cpu_watts: f64,
    pub sustained_seconds: u64,
}

impl Default for SuggestionsConfig {
    fn default() -> Self {
        Self {
            clear_after_seconds: 5,
            power_saver: PowerSaverConfig::default(),
            top_processes: TopProcessesConfig::default(),
        }
    }
}

impl Default for PowerSaverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            total_watts: 15.0,
            cpu_watts: 8.0,
            sustained_seconds: 3,
        }
    }
}

impl Default for TopProcessesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cpu_watts: 8.0,
            sustained_seconds: 10,
        }
    }
}

impl SuggestionsConfig {
    pub fn clear_after(&self) -> Duration {
        Duration::from_secs(self.clear_after_seconds)
    }

    pub fn power_saver_rules(&self) -> Vec<SuggestionRule> {
        let settings = &self.power_saver;
        if !settings.enabled {
            return Vec::new();
        }

        let power_saver = || {
            Some(ActionDescriptor {
                label: "Switch to power saver".to_string(),
                kind: ActionKind::SetPowerProfile(PowerProfile::PowerSaver),
            })
        };
        let sustained_for = Duration::from_secs(settings.sustained_seconds);

        vec![
            SuggestionRule {
                component_name: "total".to_string(),
                threshold_watts: settings.total_watts,
                sustained_for,
                message:
                    "Total power draw has been high. Consider switching to power saver profile."
                        .to_string(),
                severity: Severity::Warning,
                action: power_saver(),
            },
            SuggestionRule {
                component_name: "cpu".to_string(),
                threshold_watts: settings.cpu_watts,
                sustained_for,
                message: "CPU power draw has been high. Consider reducing workload or switching to power saver."
                    .to_string(),
                severity: Severity::Warning,
                action: power_saver(),
            },
        ]
    }

    pub fn top_processes_rules(&self) -> Vec<SuggestionRule> {
        let settings = &self.top_processes;
        if !settings.enabled {
            return Vec::new();
        }

        vec![SuggestionRule {
            component_name: "cpu".to_string(),
            threshold_watts: settings.cpu_watts,
            sustained_for: Duration::from_secs(settings.sustained_seconds),
            message: "CPU usage has been high for a while. Review top processes to identify what's consuming resources."
                .to_string(),
            severity: Severity::Info,
            action: Some(ActionDescriptor {
                label: "Show top processes".to_string(),
                kind: ActionKind::ShowTopProcesses,
            }),
        }]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    NotFound(PathBuf),
    Unreadable {
        path: PathBuf,
        message: String,
    },
    Invalid {
        path: Option<PathBuf>,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound(path) => write!(f, "config file not found: {}", path.display()),
            ConfigError::Unreadable { path, message } => {
                write!(f, "cannot read config file {}: {message}", path.display())
            }
            ConfigError::Invalid {
                path: Some(path),
                message,
            } => write!(f, "invalid config file {}: {message}", path.display()),
            ConfigError::Invalid {
                path: None,
                message,
            } => write!(f, "invalid config: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|e| ConfigError::Invalid {
            path: None,
            message: e.to_string().trim().to_string(),
        })?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let suggestions = &self.suggestions;
        let thresholds = [
            (
                "suggestions.power_saver.total_watts",
                suggestions.power_saver.total_watts,
            ),
            (
                "suggestions.power_saver.cpu_watts",
                suggestions.power_saver.cpu_watts,
            ),
            (
                "suggestions.top_processes.cpu_watts",
                suggestions.top_processes.cpu_watts,
            ),
        ];

        for (key, watts) in thresholds {
            if !(watts.is_finite() && watts > 0.0) {
                return Err(ConfigError::Invalid {
                    path: None,
                    message: format!("{key} must be a positive number of watts, got {watts}"),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Environment {
    pub config: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    pub home: Option<OsString>,
    pub appdata: Option<OsString>,
    pub exe: Option<PathBuf>,
    pub sudo_uid: Option<OsString>,
    pub sudo_gid: Option<OsString>,
    pub windows: bool,
}

impl Environment {
    pub fn current() -> Self {
        Self {
            config: std::env::var_os("POWERWATCH_CONFIG"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            home: std::env::var_os("HOME"),
            appdata: std::env::var_os("APPDATA"),
            exe: std::env::current_exe().ok(),
            sudo_uid: std::env::var_os("SUDO_UID"),
            sudo_gid: std::env::var_os("SUDO_GID"),
            windows: cfg!(windows),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSource {
    pub path: PathBuf,
    pub required: bool,
    pub create_if_missing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub notice: Option<String>,
}

fn non_empty(value: &Option<OsString>) -> Option<PathBuf> {
    value
        .as_ref()
        .filter(|text| !text.is_empty())
        .map(PathBuf::from)
}

pub fn project_root_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file() && dir.join("powerwatch-core").is_dir())
        .map(Path::to_path_buf)
}

pub fn resolve_source(flag: Option<&Path>, env: &Environment) -> Option<ConfigSource> {
    if let Some(path) = flag {
        return Some(ConfigSource {
            path: path.to_path_buf(),
            required: true,
            create_if_missing: false,
        });
    }
    if let Some(path) = non_empty(&env.config) {
        return Some(ConfigSource {
            path,
            required: true,
            create_if_missing: false,
        });
    }
    if let Some(root) = env.exe.as_deref().and_then(project_root_of) {
        return Some(ConfigSource {
            path: root.join("config.toml"),
            required: false,
            create_if_missing: true,
        });
    }

    let default_dir = if env.windows {
        non_empty(&env.appdata)
    } else {
        non_empty(&env.xdg_config_home)
            .filter(|dir| dir.is_absolute())
            .or_else(|| non_empty(&env.home).map(|home| home.join(".config")))
    }?;

    Some(ConfigSource {
        path: default_dir.join("powerwatch").join("config.toml"),
        required: false,
        create_if_missing: false,
    })
}

pub fn load(flag: Option<&Path>) -> Result<Loaded, ConfigError> {
    load_with(flag, &Environment::current())
}

pub fn load_with(flag: Option<&Path>, env: &Environment) -> Result<Loaded, ConfigError> {
    match resolve_source(flag, env) {
        Some(source) => load_source(&source, env),
        None => Ok(Loaded {
            config: Config::default(),
            notice: None,
        }),
    }
}

fn load_source(source: &ConfigSource, env: &Environment) -> Result<Loaded, ConfigError> {
    match std::fs::read_to_string(&source.path) {
        Ok(text) => Config::parse(&text)
            .map(|config| Loaded {
                config,
                notice: None,
            })
            .map_err(|error| match error {
                ConfigError::Invalid { message, .. } => ConfigError::Invalid {
                    path: Some(source.path.clone()),
                    message,
                },
                other => other,
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if source.required {
                Err(ConfigError::NotFound(source.path.clone()))
            } else if source.create_if_missing {
                create_default_file(source, env)
            } else {
                Ok(Loaded {
                    config: Config::default(),
                    notice: None,
                })
            }
        }
        Err(error) => Err(ConfigError::Unreadable {
            path: source.path.clone(),
            message: error.to_string(),
        }),
    }
}

fn create_default_file(source: &ConfigSource, env: &Environment) -> Result<Loaded, ConfigError> {
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&source.path)
        .and_then(|mut file| file.write_all(EXAMPLE_CONFIG.as_bytes()));

    match written {
        Ok(()) => {
            hand_back_to_sudo_user(&source.path, env);
            Ok(Loaded {
                config: Config::default(),
                notice: Some(format!(
                    "created {} with the default settings; edit it to change the thresholds",
                    source.path.display()
                )),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = ConfigSource {
                create_if_missing: false,
                ..source.clone()
            };
            load_source(&existing, env)
        }
        Err(error) => Ok(Loaded {
            config: Config::default(),
            notice: Some(format!(
                "could not create {}: {error}; using the default settings",
                source.path.display()
            )),
        }),
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn sudo_owner(env: &Environment) -> Option<(u32, u32)> {
    let uid = env.sudo_uid.as_ref()?.to_str()?.parse().ok()?;
    let gid = env.sudo_gid.as_ref()?.to_str()?.parse().ok()?;
    Some((uid, gid))
}

#[cfg(unix)]
fn hand_back_to_sudo_user(path: &Path, env: &Environment) {
    if let Some((uid, gid)) = sudo_owner(env) {
        let _ = std::os::unix::fs::chown(path, Some(uid), Some(gid));
    }
}

#[cfg(not(unix))]
fn hand_back_to_sudo_user(_path: &Path, _env: &Environment) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    fn error_of(text: &str) -> String {
        Config::parse(text).unwrap_err().to_string()
    }

    fn env(
        config: Option<&str>,
        xdg: Option<&str>,
        home: Option<&str>,
        appdata: Option<&str>,
        windows: bool,
    ) -> Environment {
        Environment {
            config: config.map(OsString::from),
            xdg_config_home: xdg.map(OsString::from),
            home: home.map(OsString::from),
            appdata: appdata.map(OsString::from),
            windows,
            ..Environment::default()
        }
    }

    #[test]
    fn the_defaults_are_the_thresholds_the_project_has_always_used() {
        let config = Config::default().suggestions;

        assert_eq!(config.clear_after_seconds, 5);
        assert_eq!(config.power_saver.total_watts, 15.0);
        assert_eq!(config.power_saver.cpu_watts, 8.0);
        assert_eq!(config.power_saver.sustained_seconds, 3);
        assert_eq!(config.top_processes.cpu_watts, 8.0);
        assert_eq!(config.top_processes.sustained_seconds, 10);
        assert!(config.power_saver.enabled && config.top_processes.enabled);
    }

    #[test]
    fn an_empty_file_means_the_defaults() {
        assert_eq!(parse(""), Config::default());
    }

    #[test]
    fn a_partial_file_overrides_only_what_it_names() {
        let config = parse("[suggestions.power_saver]\ncpu_watts = 20.5\n");

        assert_eq!(config.suggestions.power_saver.cpu_watts, 20.5);
        assert_eq!(config.suggestions.power_saver.total_watts, 15.0);
        assert_eq!(
            config.suggestions.top_processes,
            TopProcessesConfig::default()
        );
    }

    #[test]
    fn integers_are_accepted_for_watts() {
        let config = parse("[suggestions.power_saver]\ntotal_watts = 60\n");

        assert_eq!(config.suggestions.power_saver.total_watts, 60.0);
    }

    #[test]
    fn a_misspelled_key_is_rejected_and_named() {
        let message = error_of("[suggestions.power_saver]\ntotl_watts = 20\n");

        assert!(message.contains("totl_watts"), "{message}");
    }

    #[test]
    fn an_unknown_section_is_rejected_and_named() {
        let message = error_of("[suggestion]\nclear_after_seconds = 1\n");

        assert!(message.contains("suggestion"), "{message}");
    }

    #[test]
    fn a_value_of_the_wrong_type_is_rejected() {
        let message = error_of("[suggestions.power_saver]\ncpu_watts = \"high\"\n");

        assert!(message.contains("cpu_watts"), "{message}");
    }

    #[test]
    fn thresholds_must_be_positive_numbers_and_the_message_names_the_key() {
        for bad in ["0", "-3.5", "nan", "inf"] {
            let message = error_of(&format!("[suggestions.power_saver]\ncpu_watts = {bad}\n"));

            assert!(
                message.contains("suggestions.power_saver.cpu_watts"),
                "{bad}: {message}"
            );
            assert!(message.contains("positive"), "{bad}: {message}");
        }
        assert!(error_of("[suggestions.top_processes]\ncpu_watts = 0\n")
            .contains("suggestions.top_processes.cpu_watts"));
        assert!(error_of("[suggestions.power_saver]\ntotal_watts = -1\n")
            .contains("suggestions.power_saver.total_watts"));
    }

    #[test]
    fn the_rules_are_built_from_the_configured_values() {
        let config = parse(
            "[suggestions.power_saver]\ntotal_watts = 40\ncpu_watts = 25\nsustained_seconds = 7\n\n[suggestions.top_processes]\ncpu_watts = 30\nsustained_seconds = 20\n",
        );

        let saver = config.suggestions.power_saver_rules();
        assert_eq!(saver.len(), 2);
        assert_eq!(saver[0].component_name, "total");
        assert_eq!(saver[0].threshold_watts, 40.0);
        assert_eq!(saver[1].component_name, "cpu");
        assert_eq!(saver[1].threshold_watts, 25.0);
        for rule in &saver {
            assert_eq!(rule.sustained_for, Duration::from_secs(7));
            assert_eq!(rule.severity, Severity::Warning);
            assert_eq!(
                rule.action.as_ref().map(|a| &a.kind),
                Some(&ActionKind::SetPowerProfile(PowerProfile::PowerSaver))
            );
        }

        let top = config.suggestions.top_processes_rules();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].component_name, "cpu");
        assert_eq!(top[0].threshold_watts, 30.0);
        assert_eq!(top[0].sustained_for, Duration::from_secs(20));
        assert_eq!(
            top[0].action.as_ref().map(|a| &a.kind),
            Some(&ActionKind::ShowTopProcesses)
        );
    }

    #[test]
    fn disabling_a_group_removes_its_rules_and_keeps_the_other() {
        let config = parse("[suggestions.power_saver]\nenabled = false\n");
        assert!(config.suggestions.power_saver_rules().is_empty());
        assert_eq!(config.suggestions.top_processes_rules().len(), 1);

        let config = parse("[suggestions.top_processes]\nenabled = false\n");
        assert!(config.suggestions.top_processes_rules().is_empty());
        assert_eq!(config.suggestions.power_saver_rules().len(), 2);
    }

    #[test]
    fn clear_after_comes_from_the_config() {
        let config = parse("[suggestions]\nclear_after_seconds = 12\n");

        assert_eq!(config.suggestions.clear_after(), Duration::from_secs(12));
    }

    #[test]
    fn the_shipped_example_is_valid_and_matches_the_defaults() {
        assert_eq!(Config::parse(EXAMPLE_CONFIG).unwrap(), Config::default());
    }

    #[test]
    fn the_flag_wins_over_everything_and_is_required() {
        let environment = env(
            Some("/env.toml"),
            Some("/xdg"),
            Some("/home/me"),
            None,
            false,
        );

        let source = resolve_source(Some(Path::new("/flag.toml")), &environment).unwrap();

        assert_eq!(
            source,
            ConfigSource {
                path: PathBuf::from("/flag.toml"),
                required: true,
                create_if_missing: false
            }
        );
    }

    #[test]
    fn the_environment_variable_is_next_and_is_required() {
        let environment = env(
            Some("/env.toml"),
            Some("/xdg"),
            Some("/home/me"),
            None,
            false,
        );

        let source = resolve_source(None, &environment).unwrap();

        assert_eq!(source.path, PathBuf::from("/env.toml"));
        assert!(source.required);
    }

    #[test]
    fn the_default_location_follows_xdg_then_home_and_is_optional() {
        let xdg_dir = std::env::temp_dir().join("xdg");
        let with_xdg = env(None, xdg_dir.to_str(), Some("/home/me"), None, false);
        let source = resolve_source(None, &with_xdg).unwrap();
        assert_eq!(source.path, xdg_dir.join("powerwatch").join("config.toml"));
        assert!(!source.required);

        let home_only = env(None, None, Some("/home/me"), None, false);
        assert_eq!(
            resolve_source(None, &home_only).unwrap().path,
            PathBuf::from("/home/me/.config/powerwatch/config.toml")
        );

        let relative_xdg = env(None, Some("relative/dir"), Some("/home/me"), None, false);
        assert_eq!(
            resolve_source(None, &relative_xdg).unwrap().path,
            PathBuf::from("/home/me/.config/powerwatch/config.toml")
        );
    }

    #[test]
    fn windows_uses_appdata() {
        let environment = env(None, None, Some("C:\\Users\\me"), Some("C:\\AppData"), true);

        let source = resolve_source(None, &environment).unwrap();

        assert_eq!(
            source.path,
            PathBuf::from("C:\\AppData")
                .join("powerwatch")
                .join("config.toml")
        );
    }

    #[test]
    fn empty_variables_count_as_unset_and_nothing_means_no_file() {
        let environment = env(Some(""), Some(""), Some(""), Some(""), false);

        assert_eq!(resolve_source(None, &environment), None);
        assert_eq!(
            load_with(None, &environment).unwrap().config,
            Config::default()
        );
    }

    #[test]
    fn a_missing_default_file_means_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_str().unwrap();

        let config = load_with(None, &env(None, None, Some(home), None, false))
            .unwrap()
            .config;

        assert_eq!(config, Config::default());
    }

    #[test]
    fn a_missing_file_that_was_asked_for_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");

        let error = load_with(Some(&path), &Environment::default()).unwrap_err();

        assert_eq!(error, ConfigError::NotFound(path));
    }

    #[test]
    fn the_default_location_is_read_when_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".config/powerwatch");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "[suggestions.power_saver]\ncpu_watts = 33\n",
        )
        .unwrap();
        let home = dir.path().to_str().unwrap();

        let config = load_with(None, &env(None, None, Some(home), None, false))
            .unwrap()
            .config;

        assert_eq!(config.suggestions.power_saver.cpu_watts, 33.0);
    }

    #[test]
    fn an_invalid_file_is_reported_with_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[suggestions.power_saver]\ncpu_watts = -2\n").unwrap();

        let message = load_with(Some(&path), &Environment::default())
            .unwrap_err()
            .to_string();

        assert!(message.contains(path.to_str().unwrap()), "{message}");
        assert!(message.contains("cpu_watts"), "{message}");
    }

    fn make_project() -> (tempfile::TempDir, Environment) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::create_dir_all(dir.path().join("powerwatch-core")).unwrap();
        std::fs::create_dir_all(dir.path().join("target/release")).unwrap();
        let environment = Environment {
            exe: Some(dir.path().join("target/release/powerwatch-tui")),
            ..Environment::default()
        };
        (dir, environment)
    }

    #[test]
    fn the_project_folder_is_found_from_the_executable() {
        let (dir, environment) = make_project();

        let root = project_root_of(environment.exe.as_deref().unwrap());

        assert_eq!(root.as_deref(), Some(dir.path()));
    }

    #[test]
    fn an_executable_outside_a_project_has_no_project_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        let exe = dir.path().join("bin/powerwatch-tui");

        assert_eq!(project_root_of(&exe), None);
    }

    #[test]
    fn the_project_file_is_the_default_location_and_may_be_created() {
        let (dir, environment) = make_project();
        let environment = Environment {
            home: Some(OsString::from("/home/me")),
            ..environment
        };

        let source = resolve_source(None, &environment).unwrap();

        assert_eq!(
            source,
            ConfigSource {
                path: dir.path().join("config.toml"),
                required: false,
                create_if_missing: true
            }
        );
    }

    #[test]
    fn the_flag_and_the_variable_still_win_over_the_project_file() {
        let (_dir, environment) = make_project();

        let by_flag = resolve_source(Some(Path::new("/flag.toml")), &environment).unwrap();
        assert_eq!(by_flag.path, PathBuf::from("/flag.toml"));

        let with_variable = Environment {
            config: Some(OsString::from("/env.toml")),
            ..environment
        };
        let by_variable = resolve_source(None, &with_variable).unwrap();
        assert_eq!(by_variable.path, PathBuf::from("/env.toml"));
        assert!(!by_variable.create_if_missing);
    }

    #[test]
    fn a_missing_project_file_is_created_from_the_example_once() {
        let (dir, environment) = make_project();
        let path = dir.path().join("config.toml");

        let first = load_with(None, &environment).unwrap();

        assert_eq!(first.config, Config::default());
        assert!(first.notice.as_deref().unwrap().contains("created"));
        assert!(first
            .notice
            .as_deref()
            .unwrap()
            .contains(path.to_str().unwrap()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE_CONFIG);

        let second = load_with(None, &environment).unwrap();
        assert_eq!(second.notice, None);
    }

    #[test]
    fn an_edited_project_file_is_read_and_never_overwritten() {
        let (dir, environment) = make_project();
        let path = dir.path().join("config.toml");
        let mine = "[suggestions.power_saver]\ncpu_watts = 42\n";
        std::fs::write(&path, mine).unwrap();

        let loaded = load_with(None, &environment).unwrap();

        assert_eq!(loaded.config.suggestions.power_saver.cpu_watts, 42.0);
        assert_eq!(loaded.notice, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
    }

    #[test]
    fn an_invalid_project_file_is_an_error_not_a_silent_reset() {
        let (dir, environment) = make_project();
        std::fs::write(
            dir.path().join("config.toml"),
            "[suggestions.power_saver]\ncpu_watts = -1\n",
        )
        .unwrap();

        let error = load_with(None, &environment).unwrap_err().to_string();

        assert!(error.contains("config.toml"), "{error}");
        assert!(error.contains("cpu_watts"), "{error}");
    }

    #[test]
    fn if_the_file_cannot_be_created_the_defaults_are_used_with_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let source = ConfigSource {
            path: dir.path().join("no-such-folder/config.toml"),
            required: false,
            create_if_missing: true,
        };

        let loaded = load_source(&source, &Environment::default()).unwrap();

        assert_eq!(loaded.config, Config::default());
        assert!(loaded
            .notice
            .as_deref()
            .unwrap()
            .contains("could not create"));
    }

    #[test]
    fn a_file_created_by_another_process_in_the_meantime_is_read_not_replaced() {
        let (dir, _environment) = make_project();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[suggestions]\nclear_after_seconds = 9\n").unwrap();
        let source = ConfigSource {
            path,
            required: false,
            create_if_missing: true,
        };

        let loaded = create_default_file(&source, &Environment::default()).unwrap();

        assert_eq!(loaded.config.suggestions.clear_after_seconds, 9);
    }

    #[test]
    fn the_sudo_user_is_read_from_the_environment() {
        let sudo = |uid: Option<&str>, gid: Option<&str>| Environment {
            sudo_uid: uid.map(OsString::from),
            sudo_gid: gid.map(OsString::from),
            ..Environment::default()
        };

        assert_eq!(
            sudo_owner(&sudo(Some("1000"), Some("1001"))),
            Some((1000, 1001))
        );
        assert_eq!(sudo_owner(&sudo(Some("1000"), None)), None);
        assert_eq!(sudo_owner(&sudo(None, Some("1000"))), None);
        assert_eq!(sudo_owner(&sudo(Some("abc"), Some("1000"))), None);
        assert_eq!(sudo_owner(&sudo(Some("-1"), Some("1000"))), None);
    }
}
