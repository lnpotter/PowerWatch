use crate::suggestions::{ActionKind, PowerProfile, Suggestion};

pub fn is_redundant(
    suggestion: &Suggestion,
    current_profile: impl Fn() -> Option<PowerProfile>,
) -> bool {
    match suggestion.action.as_ref().map(|action| &action.kind) {
        Some(ActionKind::SetPowerProfile(target)) => current_profile() == Some(*target),
        _ => false,
    }
}

pub fn detect_current_profile() -> Option<PowerProfile> {
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
        use std::process::Command;
        if let Ok(output) = Command::new("powercfg").arg("/getactivescheme").output() {
            if output.status.success() {
                return parse_windows_active_scheme(&String::from_utf8_lossy(&output.stdout));
            }
        }
    }

    None
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_windows_active_scheme(text: &str) -> Option<PowerProfile> {
    let text = text.to_ascii_lowercase();
    if text.contains("a1841308-3541-4fab-bc81-f71556f20b4a") {
        Some(PowerProfile::PowerSaver)
    } else if text.contains("381b4222-f694-41f0-9685-ff5bb260df2e") {
        Some(PowerProfile::Balanced)
    } else if text.contains("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c")
        || text.contains("e9a42b02-d5df-448d-aa00-03f14749eb61")
    {
        Some(PowerProfile::Performance)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suggestions::{ActionDescriptor, Severity};

    fn suggestion_with(kind: Option<ActionKind>) -> Suggestion {
        Suggestion {
            message: "high".to_string(),
            component_name: "cpu".to_string(),
            severity: Severity::Info,
            action: kind.map(|kind| ActionDescriptor {
                label: "do it".to_string(),
                kind,
            }),
        }
    }

    #[test]
    fn switching_to_the_profile_already_in_use_is_redundant() {
        let suggestion =
            suggestion_with(Some(ActionKind::SetPowerProfile(PowerProfile::PowerSaver)));

        assert!(is_redundant(&suggestion, || Some(PowerProfile::PowerSaver)));
    }

    #[test]
    fn switching_to_a_different_or_unknown_profile_is_not_redundant() {
        let suggestion =
            suggestion_with(Some(ActionKind::SetPowerProfile(PowerProfile::PowerSaver)));

        assert!(!is_redundant(&suggestion, || Some(PowerProfile::Balanced)));
        assert!(!is_redundant(&suggestion, || None));
    }

    #[test]
    fn the_profile_is_only_looked_up_for_profile_actions() {
        let panic_if_called = || -> Option<PowerProfile> { panic!("must not detect the profile") };

        assert!(!is_redundant(
            &suggestion_with(Some(ActionKind::ShowTopProcesses)),
            panic_if_called
        ));
        assert!(!is_redundant(&suggestion_with(None), panic_if_called));
    }

    #[test]
    fn windows_scheme_guids_map_to_profiles() {
        let line = |guid: &str, name: &str| format!("Power Scheme GUID: {guid}  ({name})\n");

        assert_eq!(
            parse_windows_active_scheme(&line(
                "a1841308-3541-4fab-bc81-f71556f20b4a",
                "Power saver"
            )),
            Some(PowerProfile::PowerSaver)
        );
        assert_eq!(
            parse_windows_active_scheme(&line("381b4222-f694-41f0-9685-ff5bb260df2e", "Balanced")),
            Some(PowerProfile::Balanced)
        );
        assert_eq!(
            parse_windows_active_scheme(&line(
                "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c",
                "High performance"
            )),
            Some(PowerProfile::Performance)
        );
        assert_eq!(
            parse_windows_active_scheme(&line(
                "e9a42b02-d5df-448d-aa00-03f14749eb61",
                "Ultimate Performance"
            )),
            Some(PowerProfile::Performance)
        );
    }

    #[test]
    fn windows_scheme_is_matched_by_guid_not_by_localized_name() {
        let output = "GUID do Esquema de Energia: A1841308-3541-4FAB-BC81-F71556F20B4A  (Economia de energia)\n";

        assert_eq!(
            parse_windows_active_scheme(output),
            Some(PowerProfile::PowerSaver)
        );
    }

    #[test]
    fn a_custom_windows_scheme_is_unknown() {
        let output = "Power Scheme GUID: 11111111-2222-3333-4444-555555555555  (My plan)\n";

        assert_eq!(parse_windows_active_scheme(output), None);
        assert_eq!(parse_windows_active_scheme(""), None);
    }
}
